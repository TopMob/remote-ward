pub mod dxgi;
pub mod error;
pub mod wgc;

pub use dxgi::{AcquiredFrame, AdapterInfo, DxgiCapturer};
pub use error::CaptureError;
pub use wgc::{OnFrameCallback, WgcCaptureSession};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enumerate_adapters() {
        let adapters = DxgiCapturer::enumerate_adapters().expect("enumerate adapters failed");
        for a in &adapters {
            println!("Adapter: {} (VRAM: {} MB)", a.description, a.dedicated_video_memory / 1024 / 1024);
        }
        assert!(!adapters.is_empty());
    }

    #[test]
    fn test_dxgi_capturer() {
        match DxgiCapturer::new(0, 0) {
            Ok(capturer) => {
                println!("DXGI Capturer created: {}x{}", capturer.width, capturer.height);
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
                println!("Primary monitor: w={}, h={}", m.width().unwrap_or(0), m.height().unwrap_or(0));
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
                    println!("Monitor #{}: w={}, h={}", i, m.width().unwrap_or(0), m.height().unwrap_or(0));
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
            println!("WGC Frame arrived! tex={:?}, dev={:?}, w={}, h={}", tex, dev, w, h);
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
}
