use host_server::{HostConfig, RemoteWardHost};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info,host_server=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = HostConfig::default();
    let host = RemoteWardHost::new(config);

    host.run().await?;
    Ok(())
}
