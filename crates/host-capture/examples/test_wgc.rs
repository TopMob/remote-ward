use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
};

struct CaptureBenchmark {
    start_time: Instant,
    frame_count: u32,
    latencies: Vec<u128>,
    last_frame_instant: Instant,
}

impl GraphicsCaptureApiHandler for CaptureBenchmark {
    type Flags = ();
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(_ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        println!("Инициализация Windows Graphics Capture (WGC)...");
        let now = Instant::now();
        Ok(Self {
            start_time: now,
            frame_count: 0,
            latencies: Vec::new(),
            last_frame_instant: now,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame_instant).as_micros();
        self.last_frame_instant = now;

        if self.frame_count > 0 {
            self.latencies.push(dt);
        }
        self.frame_count += 1;

        println!(
            "Кадр #{:>2} получен: {}x{}, задержка с пред.: {:.2} мс",
            self.frame_count,
            frame.width(),
            frame.height(),
            dt as f64 / 1000.0
        );

        // Захватываем 15 кадров для теста
        if self.frame_count >= 15 {
            capture_control.stop();
        }

        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        let total_time = self.start_time.elapsed();
        println!("\n============================================================");
        println!("      РЕЗУЛЬТАТЫ ЗАХВАТА ЧЕРЕЗ WINDOWS GRAPHICS CAPTURE     ");
        println!("============================================================");
        println!("  Захвачено кадров: {}", self.frame_count);
        println!("  Общее время: {:.2} с", total_time.as_secs_f64());

        if !self.latencies.is_empty() {
            self.latencies.sort_unstable();
            let avg = self.latencies.iter().sum::<u128>() / self.latencies.len() as u128;
            let min = self.latencies[0];
            let max = *self.latencies.last().unwrap();
            let p50 = self.latencies[(self.latencies.len() * 50) / 100];
            let p95 = self.latencies[(self.latencies.len() * 95) / 100];

            println!("  Интервал между кадрами:");
            println!(
                "    Средний: {:.2} мс ({} FPS)",
                avg as f64 / 1000.0,
                1_000_000.0 / avg as f64
            );
            println!("    Медиана (p50): {:.2} мс", p50 as f64 / 1000.0);
            println!("    Минимум: {:.2} мс", min as f64 / 1000.0);
            println!("    95-й перцентиль: {:.2} мс", p95 as f64 / 1000.0);
            println!("    Максимум: {:.2} мс", max as f64 / 1000.0);
        }
        println!("============================================================");
        Ok(())
    }
}

use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== ТЕСТ ЗАХВАТА ЭКРАНА: WINDOWS GRAPHICS CAPTURE ===");

    // Получаем основной монитор
    let primary_monitor = Monitor::primary()?;
    println!("Выбран основной монитор: {:?}", primary_monitor.name()?);

    let settings = Settings::new(
        primary_monitor,
        CursorCaptureSettings::WithoutCursor,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        (),
    );

    CaptureBenchmark::start(settings)?;

    Ok(())
}
