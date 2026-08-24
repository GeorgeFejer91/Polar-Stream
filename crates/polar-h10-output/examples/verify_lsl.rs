use std::{env, path::PathBuf};

use polar_h10_core::AccSample;
use polar_h10_output::{OutputConfig, OutputRouter};
use vernier_gdx_core::{
    NumericMeasurementType, SampleEncoding, SamplingMode, SensorInfo, SensorSamples,
};

fn vernier_sensor(
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
    router.configure_vernier_streams(
        "GDX-RB",
        100_000,
        &[
            vernier_sensor(
                1,
                "Force",
                "N",
                NumericMeasurementType::Real,
                SamplingMode::Periodic,
            ),
            vernier_sensor(
                2,
                "Respiration Rate",
                "breaths/min",
                NumericMeasurementType::Real,
                SamplingMode::Aperiodic,
            ),
            vernier_sensor(
                3,
                "Steps",
                "count",
                NumericMeasurementType::Integer,
                SamplingMode::Aperiodic,
            ),
            vernier_sensor(
                4,
                "Step Rate",
                "steps/min",
                NumericMeasurementType::Real,
                SamplingMode::Aperiodic,
            ),
        ],
    )?;
    router.publish_vernier_raw(
        100_000_000,
        100_000,
        7,
        0,
        0,
        500,
        SampleEncoding::Float32,
        &[
            SensorSamples {
                sensor_number: 1,
                values: vec![12.25, 12.5],
            },
            SensorSamples {
                sensor_number: 2,
                values: vec![18.0],
            },
        ],
    );
    router.publish_vernier_breathing(100_000_000, &[0.25, 0.75], 100_000);
    let health = router.health();
    if !health.lsl.starts_with("Publishing ") {
        return Err(format!(
            "Bundled liblsl failed its Vernier outlet/sample check: {}",
            health.lsl
        ));
    }
    println!("Bundled liblsl and Vernier check passed: {}", health.lsl);
    Ok(())
}
