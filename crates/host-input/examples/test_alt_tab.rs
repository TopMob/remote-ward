use core_protocol::{ButtonState, InputEvent};
use host_input::WindowsInputInjector;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Тестирование Alt+Tab через SendInput на хосте...");
    let injector = WindowsInputInjector::new();

    // 1. Нажимаем Alt (scan code 0x38)
    println!("  -> Alt DOWN");
    injector.inject(&InputEvent::Keyboard {
        scan_code: 0x38,
        is_extended: false,
        state: ButtonState::Pressed,
    })?;

    std::thread::sleep(Duration::from_millis(100));

    // 2. Нажимаем Tab (scan code 0x0F)
    println!("  -> Tab DOWN");
    injector.inject(&InputEvent::Keyboard {
        scan_code: 0x0F,
        is_extended: false,
        state: ButtonState::Pressed,
    })?;

    std::thread::sleep(Duration::from_millis(50));

    // 3. Отпускаем Tab
    println!("  -> Tab UP");
    injector.inject(&InputEvent::Keyboard {
        scan_code: 0x0F,
        is_extended: false,
        state: ButtonState::Released,
    })?;

    std::thread::sleep(Duration::from_millis(200));

    // 4. Отпускаем Alt
    println!("  -> Alt UP");
    injector.inject(&InputEvent::Keyboard {
        scan_code: 0x38,
        is_extended: false,
        state: ButtonState::Released,
    })?;

    println!("✅ Alt+Tab успешно отправлен через SendInput!");
    Ok(())
}
