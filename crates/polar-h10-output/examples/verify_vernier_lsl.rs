//! Synthetic producer for exact consumer-side verification of the two
//! metadata-driven Vernier LSL outlets.

use std::{env, path::PathBuf, thread, time::Duration};

use polar_h10_output::{OutputConfig, OutputRouter};
use vernier_gdx_core::{
    NumericMeasurementType, SampleEncoding, SamplingMode, SensorInfo, SensorSamples,
};

const STREAM_BASE: &str = "polar_vernier_acceptance";

fn sensor(
    number: u8,
    sensor_id: u32,
    description: &str,
    unit: &str,
    numeric_type: NumericMeasurementType,
    sampling_mode: SamplingMode,
) -> SensorInfo {
    SensorInfo {
        number,
        sensor_id,
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
        .ok_or_else(|| "usage: verify_vernier_lsl <path-to-liblsl>".to_string())?;
    let router = OutputRouter::with_bundled_lsl(Some(path));
    router
        .configure(OutputConfig {
            stream_name: STREAM_BASE.into(),
            lsl_enabled: true,
            ..OutputConfig::default()
        })
        .await?;
    router.configure_vernier_streams(
        "GDX-RB",
        100_000,
        &[
            sensor(
                1,
                101,
                "Force",
                "N",
                NumericMeasurementType::Real,
                SamplingMode::Periodic,
            ),
            sensor(
                2,
                102,
                "Respiration Rate",
                "breaths/min",
                NumericMeasurementType::Real,
                SamplingMode::Aperiodic,
            ),
            sensor(
                3,
                103,
                "Steps",
                "count",
                NumericMeasurementType::Integer,
                SamplingMode::Aperiodic,
            ),
            sensor(
                4,
                104,
                "Step Rate",
                "steps/min",
                NumericMeasurementType::Real,
                SamplingMode::Aperiodic,
            ),
        ],
    )?;
    let health = router.health();
    if !health.lsl.starts_with("Publishing ") {
        return Err(format!("Vernier LSL outlets did not start: {}", health.lsl));
    }
    println!("POLAR_VERNIER_LSL_READY {}", health.lsl);

    // Give an external inlet enough time to resolve the freshly advertised
    // outlets before sending the finite bounded verification sequence.
    thread::sleep(Duration::from_secs(2));
    let mut sequence = 0_u64;
    for index in 0..40_u64 {
        let timestamp_ns = 1_000_000_000_u64.saturating_add(index * 100_000_000);
        let mut sensors = vec![SensorSamples {
            sensor_number: 1,
            values: vec![12.25 + index as f64 / 100.0],
        }];
        if index == 0 {
            sensors.push(SensorSamples {
                sensor_number: 2,
                values: vec![18.0],
            });
        }
        router.publish_vernier_raw(
            timestamp_ns,
            100_000,
            sequence,
            0,
            0,
            500,
            SampleEncoding::Float32,
            &sensors,
        );
        router.publish_vernier_breathing(
            timestamp_ns,
            &[if index.is_multiple_of(2) { 0.25 } else { 0.75 }],
            100_000,
        );
        sequence = sequence.saturating_add(1);

        if index == 5 {
            router.publish_vernier_raw(
                timestamp_ns.saturating_add(1),
                100_000,
                sequence,
                0,
                0,
                600,
                SampleEncoding::Integer32,
                &[SensorSamples {
                    sensor_number: 3,
                    values: vec![2_000_000_001.0],
                }],
            );
            sequence = sequence.saturating_add(1);
        }
        thread::sleep(Duration::from_millis(100));
    }
    thread::sleep(Duration::from_secs(2));
    println!("POLAR_VERNIER_LSL_COMPLETE");
    Ok(())
}
