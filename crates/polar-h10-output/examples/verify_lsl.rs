use std::{env, path::PathBuf};

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
    println!("Bundled liblsl check passed: {}", health.lsl);
    Ok(())
}
