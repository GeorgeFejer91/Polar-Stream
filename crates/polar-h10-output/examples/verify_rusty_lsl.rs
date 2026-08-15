use std::time::{Duration, Instant};

use polar_h10_core::AccSample;
use polar_h10_output::{OutputConfig, OutputRouter};

const STREAM_BASE: &str = "polar_rusty_backend";

#[tokio::main]
async fn main() -> Result<(), String> {
    let router = OutputRouter::new();
    let health = router
        .configure(OutputConfig {
            stream_name: STREAM_BASE.into(),
            lsl_enabled: true,
            outputs: vec!["raw_ecg".into(), "raw_acc".into()],
            ..OutputConfig::default()
        })
        .await?;
    if !health.lsl.contains("Optional Rusty LSL backend") {
        return Err(format!("Rusty LSL did not initialize: {}", health.lsl));
    }
    println!("POLAR_RUSTY_LSL_READY {STREAM_BASE}");

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(message) = router.poll_lsl() {
            return Err(message);
        }
        if router.health().lsl.contains("2 consumer(s)") {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "official consumers did not connect: {}",
                router.health().lsl
            ));
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    let ecg = [1_725_i32; 73];
    let acc = [AccSample {
        x_mg: 101,
        y_mg: -202,
        z_mg: 303,
    }; 36];
    for _ in 0..20 {
        let _ = router.publish_ecg(1, &ecg);
        let _ = router.publish_accelerometer(2, &acc);
        if let Some(message) = router.poll_lsl() {
            return Err(message);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    println!("POLAR_RUSTY_LSL_SENT {}", router.health().lsl);
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}
