use tracing_subscriber::EnvFilter;

use vessel_core::{LoggingFormat, Result, VesselError};

pub fn init(level: &str, format: LoggingFormat) -> Result<()> {
    let filter = EnvFilter::try_new(level)
        .or_else(|_| EnvFilter::try_new("info"))
        .map_err(|err| VesselError::Config(err.to_string()))?;

    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match format {
        LoggingFormat::Human => builder.with_target(true).try_init(),
        LoggingFormat::Json => builder.json().with_target(true).try_init(),
    }
    .map_err(|err| VesselError::Config(format!("logging init failed: {err}")))?;

    Ok(())
}
