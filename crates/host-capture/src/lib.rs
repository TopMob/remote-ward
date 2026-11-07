pub mod dxgi;
pub mod error;
pub mod session;
pub mod wgc;

pub use dxgi::{AcquiredFrame, AdapterInfo, DxgiCapturer};
pub use error::CaptureError;
pub use session::{CaptureBackend, ScreenCaptureSession};
pub use wgc::{OnFrameCallback, WgcCaptureSession};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enumerate_adapters() {
        let adapters = DxgiCapturer::enumerate_adapters().expect("enumerate adapters failed");
        for a in &adapters {
            println!(
                "Adapter: {} (VRAM: {} MB)",
                a.description,
                a.dedicated_video_memory / 1024 / 1024
            );
        }
        assert!(!adapters.is_empty());
    }

    #[test]
    fn test_dxgi_capturer() {
        match DxgiCapturer::new(0, 0) {
            Ok(capturer) => {
                println!(
                    "DXGI Capturer created: {}x{}",
                    capturer.width, capturer.height
                );
                match capturer.acquire_frame(1000) {
                    Ok(frame) => {
                        println!("DXGI Acquired frame: {:?}", frame.info.AccumulatedFrames);
                    }
                    Err(e) => {
                        println!("DXGI Acquire error: {:?}", e);
                    }
                }
            }
            Err(e) => {
                println!("DXGI new error: {:?}", e);
            }
        }
    }

    #[test]
    fn test_wgc_monitors() {
        use windows_capture::monitor::Monitor;
        match Monitor::primary() {
            Ok(m) => {
                println!(
                    "Primary monitor: w={}, h={}",
                    m.width().unwrap_or(0),
                    m.height().unwrap_or(0)
                );
            }
            Err(e) => {
                println!("Monitor::primary error: {:?}", e);
            }
        }
        let all = Monitor::enumerate();
        match all {
            Ok(list) => {
                println!("Total monitors: {}", list.len());
                for (i, m) in list.iter().enumerate() {
                    println!(
                        "Monitor #{}: w={}, h={}",
                        i,
                        m.width().unwrap_or(0),
                        m.height().unwrap_or(0)
                    );
                }
            }
            Err(e) => {
                println!("Monitor::enumerate error: {:?}", e);
            }
        }
    }

    #[test]
    fn test_wgc_session_capture() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        use std::time::Duration;

        let frame_count = Arc::new(AtomicU32::new(0));
        let fc = Arc::clone(&frame_count);

        println!("Starting WgcCaptureSession...");
        let session = WgcCaptureSession::start(move |tex, dev, w, h| {
            println!(
                "WGC Frame arrived! tex={:?}, dev={:?}, w={}, h={}",
                tex, dev, w, h
            );
            fc.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        match session {
            Ok(mut s) => {
                println!("WGC session started: {}x{}", s.width, s.height);
                std::thread::sleep(Duration::from_millis(1500));
                let count = frame_count.load(Ordering::SeqCst);
                println!("Frames received in 1.5s: {}", count);
                s.stop();
            }
            Err(e) => {
                println!("WGC session start failed: {:?}", e);
            }
        }
    }

    #[test]
    fn test_screen_capture_session_wgc_and_dxgi_fallback() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        use std::time::Duration;

        let frame_count = Arc::new(AtomicU32::new(0));
        let fc = Arc::clone(&frame_count);

        println!("Testing ScreenCaptureSession with automatic WGC primary / DXGI fallback...");
        let session = ScreenCaptureSession::start(move |_tex, _dev, w, h| {
            assert!(w > 0 && h > 0);
            fc.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        match session {
            Ok(mut s) => {
                println!(
                    "ScreenCaptureSession started with backend: {:?} ({}x{})",
                    s.backend, s.width, s.height
                );
                std::thread::sleep(Duration::from_millis(1000));
                let count = frame_count.load(Ordering::SeqCst);
                println!("Frames acquired in 1s: {}", count);
                s.stop();
            }
            Err(e) => {
                println!("ScreenCaptureSession start error: {:?}", e);
            }
        }

        // Тестируем прямой запуск резервного бэкенда DXGI Desktop Duplication
        let dxgi_count = Arc::new(AtomicU32::new(0));
        let dc = Arc::clone(&dxgi_count);
        match ScreenCaptureSession::start_dxgi_monitor(0, move |_tex, _dev, w, h| {
            assert!(w > 0 && h > 0);
            dc.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }) {
            Ok(mut dxgi_s) => {
                assert_eq!(dxgi_s.backend, CaptureBackend::Dxgi);
                println!(
                    "Direct DXGI fallback backend verified: {}x{}",
                    dxgi_s.width, dxgi_s.height
                );
                std::thread::sleep(Duration::from_millis(500));
                dxgi_s.stop();
            }
            Err(e) => {
                println!("DXGI direct fallback test note: {:?}", e);
            }
        }
    }
}
