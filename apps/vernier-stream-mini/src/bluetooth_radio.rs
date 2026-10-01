use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RadioStatus {
    pub state: &'static str,
}

#[cfg(target_os = "windows")]
mod platform {
    use super::RadioStatus;
    use std::future::IntoFuture;
    use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};

    async fn bluetooth_radio() -> Result<Option<Radio>, String> {
        let radios = Radio::GetRadiosAsync()
            .map_err(|error| format!("Bluetooth radio query failed: {error}"))?
            .into_future()
            .await
            .map_err(|error| format!("Bluetooth radio query failed: {error}"))?;
        for radio in radios {
            if radio.Kind().map_err(|error| error.to_string())? == RadioKind::Bluetooth {
                return Ok(Some(radio));
            }
        }
        Ok(None)
    }

    fn snapshot(radio: &Radio) -> Result<RadioStatus, String> {
        let state = match radio.State().map_err(|error| error.to_string())? {
            RadioState::On => "on",
            RadioState::Off => "off",
            RadioState::Disabled => "disabled",
            _ => "unknown",
        };
        Ok(RadioStatus { state })
    }

    pub async fn status() -> Result<RadioStatus, String> {
        match bluetooth_radio().await? {
            Some(radio) => snapshot(&radio),
            None => Ok(RadioStatus {
                state: "unavailable",
            }),
        }
    }

    pub async fn set_enabled(enabled: bool) -> Result<RadioStatus, String> {
        let radio = bluetooth_radio()
            .await?
            .ok_or("No Bluetooth radio is available to control.")?;
        let current = snapshot(&radio)?;
        if current.state == "disabled" {
            return Err("Bluetooth is disabled by hardware or Windows policy.".into());
        }
        if current.state == if enabled { "on" } else { "off" } {
            return Ok(current);
        }
        let access = Radio::RequestAccessAsync()
            .map_err(|error| format!("Bluetooth permission request failed: {error}"))?
            .into_future()
            .await
            .map_err(|error| format!("Bluetooth permission request failed: {error}"))?;
        if access != RadioAccessStatus::Allowed {
            return Err(match access {
                RadioAccessStatus::DeniedByUser => {
                    "Windows denied Bluetooth control. Change it in Windows Settings."
                }
                RadioAccessStatus::DeniedBySystem => {
                    "Windows policy blocks Bluetooth control. Change it in Windows Settings."
                }
                _ => "Windows did not grant Bluetooth control. Change it in Windows Settings.",
            }
            .into());
        }
        let requested = if enabled {
            RadioState::On
        } else {
            RadioState::Off
        };
        let result = radio
            .SetStateAsync(requested)
            .map_err(|error| format!("Bluetooth switch failed: {error}"))?
            .into_future()
            .await
            .map_err(|error| format!("Bluetooth switch failed: {error}"))?;
        if result != RadioAccessStatus::Allowed {
            return Err(
                "Windows did not allow the Bluetooth switch. Change it in Windows Settings.".into(),
            );
        }
        snapshot(&radio)
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::RadioStatus;

    pub async fn status() -> Result<RadioStatus, String> {
        Ok(RadioStatus {
            state: "unsupported",
        })
    }

    pub async fn set_enabled(_enabled: bool) -> Result<RadioStatus, String> {
        Err("Bluetooth radio control is currently available only on Windows.".into())
    }
}

pub use platform::{set_enabled, status};
