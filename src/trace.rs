use tracing_subscriber::{EnvFilter, fmt};

pub fn init(verbose: u8) {
    let filter = match verbose {
        0 => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        1 => EnvFilter::new("debug"),
        _ => EnvFilter::new("trace"),
    };

    fmt().with_env_filter(filter).with_target(false).init();
}

pub fn init_to_file(verbose: u8, path: &std::path::Path) -> anyhow::Result<()> {
    let filter = match verbose {
        0 => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        1 => EnvFilter::new("debug"),
        _ => EnvFilter::new("trace"),
    };

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;

    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(file)
        .init();

    Ok(())
}
