use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::Serialize;

use crate::error::{CommandError, CommandResult};

pub(crate) const LAB_RECORDER_VERSION: &str = "1.17.0";

#[derive(Clone, Debug)]
enum LaunchKind {
    Direct,
    MacApp,
}

#[derive(Clone, Debug)]
pub(crate) struct LabRecorderInstallation {
    target: PathBuf,
    executable: PathBuf,
    config: PathBuf,
    working_directory: PathBuf,
    environment: Vec<(OsString, OsString)>,
    launch_kind: LaunchKind,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LabRecorderCapability {
    pub(crate) available: bool,
    pub(crate) version: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LabRecorderLaunch {
    version: &'static str,
    message: &'static str,
}

impl LabRecorderInstallation {
    pub(crate) fn locate(resource_directory: &Path) -> Option<Self> {
        Self::locate_for_platform(resource_directory, std::env::consts::OS)
    }

    fn locate_for_platform(resource_directory: &Path, platform: &str) -> Option<Self> {
        let root = resource_directory.join("lab-recorder");
        let config = root.join("PolarStream-LabRecorder.cfg");
        let (target, executable, working_directory, environment, launch_kind) = match platform {
            "windows" => {
                let executable = root.join("LabRecorder.exe");
                (
                    executable.clone(),
                    executable,
                    root.clone(),
                    Vec::new(),
                    LaunchKind::Direct,
                )
            }
            "linux" => {
                let executable = root.join("LabRecorder");
                let library_directory = root.join("lib");
                let plugin_directory = root.join("plugins");
                let environment = vec![
                    (
                        OsString::from("LD_LIBRARY_PATH"),
                        library_directory.into_os_string(),
                    ),
                    (
                        OsString::from("QT_PLUGIN_PATH"),
                        plugin_directory.clone().into_os_string(),
                    ),
                    (
                        OsString::from("QT_QPA_PLATFORM_PLUGIN_PATH"),
                        plugin_directory.join("platforms").into_os_string(),
                    ),
                ];
                (
                    executable.clone(),
                    executable,
                    root.clone(),
                    environment,
                    LaunchKind::Direct,
                )
            }
            "macos" => {
                let bundle = root.join("LabRecorder.app");
                let executable = bundle.join("Contents/MacOS/LabRecorder");
                (
                    bundle.clone(),
                    executable,
                    root,
                    Vec::new(),
                    LaunchKind::MacApp,
                )
            }
            _ => return None,
        };
        if !config.is_file()
            || !executable.is_file()
            || matches!(launch_kind, LaunchKind::MacApp) && !target.is_dir()
        {
            return None;
        }
        Some(Self {
            target,
            executable,
            config,
            working_directory,
            environment,
            launch_kind,
        })
    }

    pub(crate) fn capability(installation: Option<&Self>) -> LabRecorderCapability {
        LabRecorderCapability {
            available: installation.is_some(),
            version: LAB_RECORDER_VERSION,
        }
    }

    pub(crate) fn open(&self) -> CommandResult<LabRecorderLaunch> {
        let mut command = match self.launch_kind {
            LaunchKind::Direct => {
                let mut command = Command::new(&self.executable);
                command.arg("-c").arg(&self.config);
                command
            }
            LaunchKind::MacApp => {
                let mut command = Command::new("/usr/bin/open");
                command
                    .arg(&self.target)
                    .arg("--args")
                    .arg("-c")
                    .arg(&self.config);
                command
            }
        };
        command
            .current_dir(&self.working_directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (name, value) in &self.environment {
            command.env(name, value);
        }
        let mut child = command.spawn().map_err(|_| {
            CommandError::new(
                "LAB_RECORDER_LAUNCH_FAILED",
                "The bundled LabRecorder could not be opened. Reinstall Polar Stream and try again.",
                true,
            )
        })?;
        let _ = std::thread::Builder::new()
            .name("lab-recorder-reaper".into())
            .spawn(move || {
                let _ = child.wait();
            });
        Ok(LabRecorderLaunch {
            version: LAB_RECORDER_VERSION,
            message: "LabRecorder opened. Choose the LSL streams to record, then save the session as XDF.",
        })
    }
}

pub(crate) fn unavailable_error() -> CommandError {
    CommandError::new(
        "LAB_RECORDER_UNAVAILABLE",
        "This build does not contain the bundled LabRecorder. Reinstall Polar Stream from the latest native package.",
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root() -> PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test clock must follow the Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "polar-stream-lab-recorder-{}-{suffix}",
            std::process::id()
        ))
    }

    fn create_file(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("test file must have a parent"))
            .expect("test directory must be created");
        std::fs::write(path, b"fixture").expect("test file must be written");
    }

    #[test]
    fn locator_accepts_only_complete_fixed_platform_layouts() {
        let resource_directory = test_root();
        let root = resource_directory.join("lab-recorder");
        create_file(&root.join("PolarStream-LabRecorder.cfg"));

        assert!(
            LabRecorderInstallation::locate_for_platform(&resource_directory, "windows").is_none()
        );
        create_file(&root.join("LabRecorder.exe"));
        let windows = LabRecorderInstallation::locate_for_platform(&resource_directory, "windows")
            .expect("complete Windows layout must be accepted");
        assert_eq!(windows.executable, root.join("LabRecorder.exe"));
        assert!(windows.environment.is_empty());

        create_file(&root.join("LabRecorder"));
        let linux = LabRecorderInstallation::locate_for_platform(&resource_directory, "linux")
            .expect("complete Linux layout must be accepted");
        assert_eq!(linux.executable, root.join("LabRecorder"));
        assert_eq!(linux.environment.len(), 3);

        create_file(&root.join("LabRecorder.app/Contents/MacOS/LabRecorder"));
        let macos = LabRecorderInstallation::locate_for_platform(&resource_directory, "macos")
            .expect("complete macOS layout must be accepted");
        assert_eq!(macos.target, root.join("LabRecorder.app"));

        assert!(
            LabRecorderInstallation::locate_for_platform(&resource_directory, "android").is_none()
        );
        std::fs::remove_dir_all(resource_directory).expect("test directory must be removed");
    }

    #[test]
    fn capability_never_claims_an_absent_bundle() {
        let capability = LabRecorderInstallation::capability(None);
        assert!(!capability.available);
        assert_eq!(capability.version, LAB_RECORDER_VERSION);
    }
}
