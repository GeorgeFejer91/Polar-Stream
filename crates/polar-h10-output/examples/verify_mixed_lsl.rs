//! Synthetic mixed-device producer for official-inlet verification of two
//! independent source routers publishing concurrently.

use std::{env, path::PathBuf, thread, time::Duration};

use polar_h10_core::AccSample;
use polar_h10_output::{OutputConfig, OutputRouter};
use vernier_gdx_core::{
    NumericMeasurementType, SampleEncoding, SamplingMode, SensorInfo, SensorSamples,
};

const BASE: &str = "polar_mixed_acceptance";
const POLAR_BASE: &str = "polar_mixed_acceptance_source-1";
const VERNIER_BASE: &str = "polar_mixed_acceptance_source-2";

fn sensor(
    number: u8,
    description: &str,
    unit: &str,
    numeric_type: NumericMeasurementType,
    sampling_mode: SamplingMode,
) -> SensorInfo {
    SensorInfo {
        number,
        sensor_id: u32::from(number),
        numeric_type,
        sampling_mode,
        description: description.into(),
        unit: unit.into(),
        uncertainty: 0.01,
        minimum: 0.0,
        maximum: 100.0,
        minimum_period_us: 100_000,
        maximum_period_us: 60_000_000,
        typical_period_us: 100_000,
        period_granularity_us: 1_000,
        mutual_exclusion_mask: 0,
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let path = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| "usage: verify_mixed_lsl <path-to-liblsl>".to_string())?;
    let polar = OutputRouter::with_bundled_lsl(Some(path.clone()));
    let vernier = OutputRouter::with_bundled_lsl(Some(path));

    polar
        .configure(OutputConfig {
            stream_name: POLAR_BASE.into(),
            lsl_enabled: true,
            outputs: vec!["raw_ecg".into(), "raw_acc".into()],
            ..OutputConfig::default()
        })
        .await?;
    vernier
        .configure(OutputConfig {
            stream_name: VERNIER_BASE.into(),
            lsl_enabled: true,
            outputs: Vec::new(),
            ..OutputConfig::default()
        })
        .await?;
    vernier.configure_vernier_streams(
        "GDX-RB",
        100_000,
        &[
            sensor(
                1,
                "Force",
                "N",
                NumericMeasurementType::Real,
                SamplingMode::Periodic,
            ),
            sensor(
                2,
                "Respiration Rate",
                "breaths/min",
                NumericMeasurementType::Real,
                SamplingMode::Aperiodic,
            ),
        ],
    )?;

    let polar_health = polar.health();
    let vernier_health = vernier.health();
    if !polar_health.lsl.starts_with("Publishing ")
        || !vernier_health.lsl.starts_with("Publishing ")
    {
        return Err(format!(
            "Mixed LSL outlets did not start (Polar: {}; Vernier: {})",
            polar_health.lsl, vernier_health.lsl
        ));
    }
    println!(
        "POLAR_MIXED_LSL_READY {BASE} Polar={} Vernier={}",
        polar_health.lsl, vernier_health.lsl
    );

    // Let four independently resolved official inlets open before the bounded
    // interleaved publication. Both routers stay live for the complete loop.
    thread::sleep(Duration::from_secs(2));
    for index in 0..50_u64 {
        let timestamp_ns = 1_000_000_000_u64.saturating_add(index * 100_000_000);
        let ecg = (0..13)
            .map(|sample| ((index * 13 + sample) as i32 % 400) - 200)
            .collect::<Vec<_>>();
        let acc = (0..20)
            .map(|sample| AccSample {
                x_mg: (index as i16).saturating_add(sample),
                y_mg: 1_000_i16.saturating_sub(sample),
                z_mg: 500_i16.saturating_add(sample),
            })
            .collect::<Vec<_>>();
        let _ = polar.publish_ecg(timestamp_ns, &ecg);
        let _ = polar.publish_accelerometer(timestamp_ns, &acc);

        let force = 12.0 + (index as f64 / 6.0).sin();
        vernier.publish_vernier_raw(
            timestamp_ns,
            100_000,
            index,
            0,
            0,
            500,
            SampleEncoding::Float32,
            &[SensorSamples {
                sensor_number: 1,
                values: vec![force],
            }],
        );
        vernier.publish_vernier_breathing(
            timestamp_ns,
            &[0.5 + 0.4 * (index as f32 / 6.0).sin()],
            100_000,
        );
        thread::sleep(Duration::from_millis(100));
    }
    thread::sleep(Duration::from_secs(2));
    println!("POLAR_MIXED_LSL_COMPLETE");
    Ok(())
}
