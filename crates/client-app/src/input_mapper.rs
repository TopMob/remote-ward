use winit::keyboard::KeyCode;

/// Преобразование физической клавиши Winit в аппаратный Windows Scan Code (100% поддержка DirectInput/RawInput в играх)
pub fn keycode_to_scancode(key: KeyCode) -> Option<(u16, bool)> {
    match key {
        // Буквы QWERTY
        KeyCode::KeyA => Some((0x1E, false)),
        KeyCode::KeyB => Some((0x30, false)),
        KeyCode::KeyC => Some((0x2E, false)),
        KeyCode::KeyD => Some((0x20, false)),
        KeyCode::KeyE => Some((0x12, false)),
        KeyCode::KeyF => Some((0x21, false)),
        KeyCode::KeyG => Some((0x22, false)),
        KeyCode::KeyH => Some((0x23, false)),
        KeyCode::KeyI => Some((0x17, false)),
        KeyCode::KeyJ => Some((0x24, false)),
        KeyCode::KeyK => Some((0x25, false)),
        KeyCode::KeyL => Some((0x26, false)),
        KeyCode::KeyM => Some((0x32, false)),
        KeyCode::KeyN => Some((0x31, false)),
        KeyCode::KeyO => Some((0x18, false)),
        KeyCode::KeyP => Some((0x19, false)),
        KeyCode::KeyQ => Some((0x10, false)),
        KeyCode::KeyR => Some((0x13, false)),
        KeyCode::KeyS => Some((0x1F, false)),
        KeyCode::KeyT => Some((0x14, false)),
        KeyCode::KeyU => Some((0x16, false)),
        KeyCode::KeyV => Some((0x2F, false)),
        KeyCode::KeyW => Some((0x11, false)),
        KeyCode::KeyX => Some((0x2D, false)),
        KeyCode::KeyY => Some((0x15, false)),
        KeyCode::KeyZ => Some((0x2C, false)),

        // Цифры основного ряда
        KeyCode::Digit0 => Some((0x0B, false)),
        KeyCode::Digit1 => Some((0x02, false)),
        KeyCode::Digit2 => Some((0x03, false)),
        KeyCode::Digit3 => Some((0x04, false)),
        KeyCode::Digit4 => Some((0x05, false)),
        KeyCode::Digit5 => Some((0x06, false)),
        KeyCode::Digit6 => Some((0x07, false)),
        KeyCode::Digit7 => Some((0x08, false)),
        KeyCode::Digit8 => Some((0x09, false)),
        KeyCode::Digit9 => Some((0x0A, false)),

        // Служебные и игровые клавиши
        KeyCode::Escape => Some((0x01, false)),
        KeyCode::Space => Some((0x39, false)),
        KeyCode::Enter => Some((0x1C, false)),
        KeyCode::Tab => Some((0x0F, false)),
        KeyCode::Backspace => Some((0x0E, false)),
        KeyCode::ShiftLeft => Some((0x2A, false)),
        KeyCode::ShiftRight => Some((0x36, false)),
        KeyCode::ControlLeft => Some((0x1D, false)),
        KeyCode::ControlRight => Some((0x1D, true)), // Extended
        KeyCode::AltLeft => Some((0x38, false)),
        KeyCode::AltRight => Some((0x38, true)), // Extended
        KeyCode::CapsLock => Some((0x3A, false)),

        // Стрелки (Extended)
        KeyCode::ArrowUp => Some((0x48, true)),
        KeyCode::ArrowDown => Some((0x50, true)),
        KeyCode::ArrowLeft => Some((0x4B, true)),
        KeyCode::ArrowRight => Some((0x4D, true)),

        // Навигация (Extended)
        KeyCode::Insert => Some((0x52, true)),
        KeyCode::Delete => Some((0x53, true)),
        KeyCode::Home => Some((0x47, true)),
        KeyCode::End => Some((0x4F, true)),
        KeyCode::PageUp => Some((0x49, true)),
        KeyCode::PageDown => Some((0x51, true)),

        // Функциональные клавиши F1-F12
        KeyCode::F1 => Some((0x3B, false)),
        KeyCode::F2 => Some((0x3C, false)),
        KeyCode::F3 => Some((0x3D, false)),
        KeyCode::F4 => Some((0x3E, false)),
        KeyCode::F5 => Some((0x3F, false)),
        KeyCode::F6 => Some((0x40, false)),
        KeyCode::F7 => Some((0x41, false)),
        KeyCode::F8 => Some((0x42, false)),
        KeyCode::F9 => Some((0x43, false)),
        KeyCode::F10 => Some((0x44, false)),
        KeyCode::F11 => Some((0x57, false)),
        KeyCode::F12 => Some((0x58, false)),

        _ => None,
    }
}
