pub mod app;
pub mod audio_player;
pub mod calibration;
pub mod config;
pub mod decoder;
pub mod discovery;
pub mod egui_painter;
pub mod input_mapper;
pub mod keyboard_hook;
pub mod launcher;
pub mod renderer;

pub use app::{run_client, ClientConfig, RemoteWardClientApp};
pub use audio_player::WasapiAudioPlayer;
pub use config::AppConfig;
pub use decoder::{DecodeError, DecodedFrame, MftVideoDecoder};
pub use discovery::{DiscoveredHost, LanDiscovery};
pub use egui_painter::D3D11EguiPainter;
pub use input_mapper::keycode_to_scancode;
pub use keyboard_hook::SystemKeyboardHook;
pub use launcher::{LauncherAction, LauncherTab, LauncherUI};
pub use renderer::D3D11Renderer;

#[cfg(test)]
mod tests {
    use super::app::nv12_to_bgra;

    #[test]
    fn test_nv12_to_bgra_conversion() {
        let width = 4usize;
        let height = 4usize;
        let y_size = width * height; // 16
        let uv_size = width * height / 2; // 8
        let mut nv12 = vec![128u8; y_size + uv_size];
        // Заполняем Y белым (255), U/V нейтральным (128)
        for i in 0..y_size {
            nv12[i] = 255;
        }

        let mut bgra = vec![0u8; width * height * 4];
        nv12_to_bgra(&nv12, width, height, &mut bgra);

        // Для чисто белого Y=255, U=128, V=128 каналы R, G, B должны быть около 255, а Alpha = 255
        for i in (0..bgra.len()).step_by(4) {
            let b = bgra[i];
            let g = bgra[i + 1];
            let r = bgra[i + 2];
            let a = bgra[i + 3];
            assert!(b >= 240, "Expected white B >= 240, got {}", b);
            assert!(g >= 240, "Expected white G >= 240, got {}", g);
            assert!(r >= 240, "Expected white R >= 240, got {}", r);
            assert_eq!(a, 255, "Alpha must be 255");
        }
    }

    #[test]
    fn test_nv12_to_bgra_black() {
        let width = 4usize;
        let height = 4usize;
        let y_size = width * height;
        let uv_size = width * height / 2;
        let mut nv12 = vec![128u8; y_size + uv_size];
        // Заполняем Y черным (0)
        for i in 0..y_size {
            nv12[i] = 0;
        }

        let mut bgra = vec![255u8; width * height * 4];
        nv12_to_bgra(&nv12, width, height, &mut bgra);

        for i in (0..bgra.len()).step_by(4) {
            let b = bgra[i];
            let g = bgra[i + 1];
            let r = bgra[i + 2];
            let a = bgra[i + 3];
            assert!(b <= 15, "Expected black B <= 15, got {}", b);
            assert!(g <= 15, "Expected black G <= 15, got {}", g);
            assert!(r <= 15, "Expected black R <= 15, got {}", r);
            assert_eq!(a, 255, "Alpha must be 255");
        }
    }
}



