use std::{env, path::PathBuf};

use polar_h10_core::AccSample;
use polar_h10_output::{OutputConfig, OutputRouter};

#[tokio::main]
async fn main() -> Result<(), String> {
    let path = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| "usage: verify_lsl <path-to-liblsl>".to_string())?;
    let router = OutputRouter::with_bundled_lsl(Some(path));
    let health = router
        .configure(OutputConfig {
            lsl_enabled: true,
            ..OutputConfig::default()
        })
        .await?;
    if !health.lsl.starts_with("Publishing ") {
        return Err(format!(
            "Bundled liblsl failed its outlet check: {}",
            health.lsl
        ));
    }
    // Exercise the exact immediate notification-sized chunk paths used by the
    // sensor coordinator, not only outlet construction.
    let _ = router.publish_ecg(1, &[12, 18, -4, 9]);
    let _ = router.publish_accelerometer(
        2,
        &[
            AccSample {
                x_mg: 1,
                y_mg: 2,
                z_mg: 1_000,
            },
            AccSample {
                x_mg: 3,
                y_mg: 4,
                z_mg: 998,
            },
        ],
    );
    println!("Bundled liblsl check passed: {}", health.lsl);
    Ok(())
}
