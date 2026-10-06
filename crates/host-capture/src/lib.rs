pub mod dxgi;
pub mod error;
pub mod wgc;

pub use dxgi::{AcquiredFrame, AdapterInfo, DxgiCapturer};
pub use error::CaptureError;
pub use wgc::{OnFrameCallback, WgcCaptureSession};
