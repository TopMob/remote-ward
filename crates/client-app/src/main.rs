use client_app::{run_client, AppConfig};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _timer = core_transport::ScopedTimerPeriod::set(1);

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

    let mut app_config = AppConfig::load();

    // Если передан адрес хоста в аргументах (например, из client.bat 192.168.1.4), сохраняем в конфиг
    if let Some(host_arg) = std::env::args().nth(1) {
        let host_arg = host_arg.trim();
        if !host_arg.is_empty() {
            app_config.record_ip(host_arg);
        }
    }

    run_client(app_config).await?;
    Ok(())
}
