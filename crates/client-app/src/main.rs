use client_app::{run_client, ClientConfig};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info,client_app=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Читаем адрес хоста из аргументов (IP или IP:PORT, по умолчанию 127.0.0.1:48001)
    let host_arg = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:48001".into());
    let host_addr: std::net::SocketAddr = if host_arg.contains(':') {
        host_arg.parse().unwrap_or_else(|_| "127.0.0.1:48001".parse().unwrap())
    } else {
        format!("{}:48001", host_arg).parse().unwrap_or_else(|_| "127.0.0.1:48001".parse().unwrap())
    };

    let config = ClientConfig {
        host_control_addr: host_addr,
        ..Default::default()
    };

    run_client(config).await?;
    Ok(())
}
