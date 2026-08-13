use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

#[cfg(unix)]
use std::fs::File;

use polar_h10_output::OutputConfig;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

const SCHEMA_VERSION: u16 = 1;
const MAX_PREFERENCES_BYTES: u64 = 1_048_576;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavedDevice {
    pub(crate) id: String,
    pub(crate) name: String,
}

impl SavedDevice {
    pub(crate) fn is_valid(&self) -> bool {
        !self.id.is_empty()
            && !self.name.is_empty()
            && self.id.len() <= 512
            && self.name.len() <= 512
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreferencesSnapshot {
    pub(crate) schema_version: u16,
    pub(crate) output_config: OutputConfig,
    pub(crate) last_device: Option<SavedDevice>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct PreferencesFile {
    schema_version: u16,
    output_config: OutputConfig,
    last_device: Option<SavedDevice>,
}

impl Default for PreferencesFile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            output_config: OutputConfig::default(),
            last_device: None,
        }
    }
}

impl From<PreferencesFile> for PreferencesSnapshot {
    fn from(value: PreferencesFile) -> Self {
        Self {
            schema_version: value.schema_version,
            output_config: value.output_config,
            last_device: value.last_device,
        }
    }
}

/// Native settings have exactly one owner. `write_gate` serializes the rare
/// save operations, while readers only take a short synchronous snapshot lock.
pub(crate) struct PreferencesStore {
    path: PathBuf,
    state: RwLock<PreferencesFile>,
    has_saved_preferences: AtomicBool,
    write_gate: Mutex<()>,
}

impl PreferencesStore {
    pub(crate) fn load(path: PathBuf) -> Self {
        let loaded = read_preferences(&path)
            .or_else(|| read_preferences(&path.with_extension("json.bak")))
            .or_else(|| read_preferences(&path.with_extension("json.tmp")));
        let has_saved_preferences = loaded.is_some();
        Self {
            path,
            state: RwLock::new(loaded.unwrap_or_default()),
            has_saved_preferences: AtomicBool::new(has_saved_preferences),
            write_gate: Mutex::new(()),
        }
    }

    pub(crate) fn has_saved_preferences(&self) -> bool {
        self.has_saved_preferences.load(Ordering::Acquire)
    }

    pub(crate) fn snapshot(&self) -> PreferencesSnapshot {
        self.state
            .read()
            .map(|state| state.clone().into())
            .unwrap_or_else(|_| PreferencesFile::default().into())
    }

    pub(crate) async fn save_output_config(
        &self,
        output_config: OutputConfig,
    ) -> Result<(), String> {
        self.update(move |preferences| preferences.output_config = output_config)
            .await
    }

    pub(crate) async fn save_last_device(&self, last_device: SavedDevice) -> Result<(), String> {
        self.update(move |preferences| preferences.last_device = Some(last_device))
            .await
    }

    pub(crate) async fn migrate_legacy(
        &self,
        output_config: Option<OutputConfig>,
        last_device: Option<SavedDevice>,
    ) -> Result<PreferencesSnapshot, String> {
        let _write = self.write_gate.lock().await;
        if self.has_saved_preferences() {
            return Ok(self.snapshot());
        }
        self.persist_update(move |preferences| {
            if let Some(output_config) = output_config {
                preferences.output_config = output_config;
            }
            if last_device.is_some() {
                preferences.last_device = last_device;
            }
        })
        .await?;
        Ok(self.snapshot())
    }

    async fn update(&self, mutate: impl FnOnce(&mut PreferencesFile)) -> Result<(), String> {
        let _write = self.write_gate.lock().await;
        self.persist_update(mutate).await
    }

    async fn persist_update(
        &self,
        mutate: impl FnOnce(&mut PreferencesFile),
    ) -> Result<(), String> {
        let mut next = self
            .state
            .read()
            .map_err(|_| "Preferences state is unavailable.".to_string())?
            .clone();
        mutate(&mut next);
        next.schema_version = SCHEMA_VERSION;

        let path = self.path.clone();
        let persisted = next.clone();
        tauri::async_runtime::spawn_blocking(move || write_atomic(&path, &persisted))
            .await
            .map_err(|error| format!("Preferences writer stopped unexpectedly: {error}"))??;
        *self
            .state
            .write()
            .map_err(|_| "Preferences state is unavailable.".to_string())? = next;
        self.has_saved_preferences.store(true, Ordering::Release);
        Ok(())
    }
}

fn read_preferences(path: &Path) -> Option<PreferencesFile> {
    if fs::metadata(path).ok()?.len() > MAX_PREFERENCES_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let mut stored: PreferencesFile = serde_json::from_slice(&bytes).ok()?;
    if stored.schema_version > SCHEMA_VERSION {
        return None;
    }
    // Unknown metrics from an older build are removed during migration. Live
    // renderer submissions are validated more strictly by OutputRouter.
    stored.output_config = stored.output_config.migrated().ok()?;
    stored.last_device = stored.last_device.filter(SavedDevice::is_valid);
    stored.schema_version = SCHEMA_VERSION;
    Some(stored)
}

fn write_atomic(path: &Path, preferences: &PreferencesFile) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Preferences path has no parent directory.".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create the preferences directory: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(preferences)
        .map_err(|error| format!("Could not encode preferences: {error}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| format!("Could not open the temporary preferences file: {error}"))?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("Could not write preferences: {error}"))?;
    drop(file);
    replace_file(&temporary, path)?;

    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("Could not finalize preferences: {error}"))?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn replace_file(temporary: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(temporary, destination)
        .map_err(|error| format!("Could not replace preferences: {error}"))
}

#[cfg(target_os = "windows")]
fn replace_file(temporary: &Path, destination: &Path) -> Result<(), String> {
    let backup = destination.with_extension("json.bak");
    let _ = fs::remove_file(&backup);
    if destination.exists() {
        fs::rename(destination, &backup)
            .map_err(|error| format!("Could not stage existing preferences: {error}"))?;
    }
    match fs::rename(temporary, destination) {
        Ok(()) => {
            let _ = fs::remove_file(backup);
            Ok(())
        }
        Err(error) => {
            if backup.exists() {
                let _ = fs::rename(&backup, destination);
            }
            Err(format!("Could not replace preferences: {error}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "polar-stream-{name}-{}-{nonce}.json",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn saves_and_reloads_native_preferences() {
        let path = test_path("round-trip");
        let store = PreferencesStore::load(path.clone());
        store
            .save_last_device(SavedDevice {
                id: "device-7".into(),
                name: "Polar H10 1234".into(),
            })
            .await
            .unwrap();
        let config = OutputConfig {
            stream_name: "participant_07".into(),
            ..OutputConfig::default()
        };
        store.save_output_config(config).await.unwrap();

        let loaded = PreferencesStore::load(path.clone()).snapshot();
        assert_eq!(loaded.schema_version, SCHEMA_VERSION);
        assert_eq!(loaded.output_config.stream_name, "participant_07");
        assert_eq!(loaded.last_device.unwrap().id, "device-7");
        let _ = fs::remove_file(path);
    }

    #[tokio::test]
    async fn imports_legacy_preferences_only_once() {
        let path = test_path("legacy");
        let store = PreferencesStore::load(path.clone());
        assert!(!store.has_saved_preferences());
        let first = store
            .migrate_legacy(
                Some(OutputConfig {
                    stream_name: "legacy_name".into(),
                    ..OutputConfig::default()
                }),
                Some(SavedDevice {
                    id: "legacy-device".into(),
                    name: "Polar H10 Legacy".into(),
                }),
            )
            .await
            .unwrap();
        assert_eq!(first.output_config.stream_name, "legacy_name");
        assert_eq!(first.last_device.unwrap().id, "legacy-device");
        assert!(store.has_saved_preferences());

        let second = store
            .migrate_legacy(Some(OutputConfig::default()), None)
            .await
            .unwrap();
        assert_eq!(second.output_config.stream_name, "legacy_name");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn corrupt_preferences_fall_back_safely() {
        let path = test_path("corrupt");
        fs::write(&path, b"{broken").unwrap();
        let snapshot = PreferencesStore::load(path.clone()).snapshot();
        assert_eq!(snapshot.schema_version, SCHEMA_VERSION);
        assert_eq!(snapshot.output_config.outputs, ["raw_ecg", "raw_acc"]);
        let _ = fs::remove_file(path);
    }
}
