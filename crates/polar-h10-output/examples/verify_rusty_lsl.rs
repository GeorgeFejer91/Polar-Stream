use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use polar_h10_core::AccSample;
use polar_h10_output::{OutputConfig, OutputRouter};

const STREAM_BASE: &str = "polar_rusty_backend";

#[tokio::main]
async fn main() -> Result<(), String> {
    let router = Arc::new(OutputRouter::new());
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

    let poll_stop = Arc::new(AtomicBool::new(false));
    let poll_router = router.clone();
    let poll_task = {
        let stop = poll_stop.clone();
        tokio::task::spawn_blocking(move || {
            while !stop.load(Ordering::Acquire) {
                if let Some(message) = poll_router.poll_lsl() {
                    eprintln!("POLAR_RUSTY_LSL_WARNING {message}");
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        })
    };

    let result = exercise_official_consumers(&router).await;
    poll_stop.store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(2), poll_task)
        .await
        .map_err(|_| "Rusty LSL poll worker did not stop within two seconds".to_owned())?
        .map_err(|error| format!("Rusty LSL poll worker failed: {error}"))?;
    result
}

async fn exercise_official_consumers(router: &OutputRouter) -> Result<(), String> {
    let ecg = [1_725_i32; 73];
    let acc = [AccSample {
        x_mg: 101,
        y_mg: -202,
        z_mg: 303,
    }; 36];

    // Match the physical verifier: the native source can produce several
    // notification-sized batches before the official inlets are opened.
    for _ in 0..20 {
        let _ = router.publish_ecg(1, &ecg);
        let _ = router.publish_accelerometer(2, &acc);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    println!("POLAR_RUSTY_LSL_READY {STREAM_BASE}");

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if router.health().lsl.contains("2 consumer(s)") {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "official consumers did not connect: {}",
                router.health().lsl
            ));
        }
        let _ = router.publish_ecg(1, &ecg);
        let _ = router.publish_accelerometer(2, &acc);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    for _ in 0..20 {
        let _ = router.publish_ecg(1, &ecg);
        let _ = router.publish_accelerometer(2, &acc);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    println!("POLAR_RUSTY_LSL_SENT {}", router.health().lsl);
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}
