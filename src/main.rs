mod config;
mod dnsmasq;
mod docker;
mod git;
mod namecom;
mod nginx;
mod state;

use anyhow::{bail, Result};
use tokio::sync::Mutex;
use tracing::info;
use tracing_subscriber::{filter::Targets, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    let filter: Targets = std::env::var("SONGREN_LOG")
        .unwrap_or_else(|_| "songren=info".into())
        .parse()
        .unwrap_or_else(|_| Targets::new().with_default(tracing::Level::INFO));

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(filter)
        .init();

    let mut args = std::env::args().skip(1);
    let Some(base_dir) = args.next() else {
        bail!("Usage: songren <dir>");
    };
    if args.next().is_some() {
        bail!("Usage: songren <dir>");
    }

    let global_config = state::load_global_config(&base_dir)?;
    state::ensure_dirs(&global_config)?;

    let _sync_lock = Mutex::new(());
    let services = state::list_services(&global_config)?;
    info!(
        listen_on = %global_config.listen_on,
        count = services.len(),
        "Songren initialized"
    );

    Ok(())
}
