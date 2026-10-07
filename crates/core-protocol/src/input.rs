use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseButton {
    Left = 1,
    Right = 2,
    Middle = 3,
    X1 = 4,
    X2 = 5,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ButtonState {
    Released = 0,
    Pressed = 1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    /// Относительное движение мыши (для игр с захватом курсора)
    MouseMoveRelative { dx: i32, dy: i32 },

    /// Абсолютное движение мыши (для обычного рабочего стола, координаты в пикселях целевого дисплея)
    MouseMoveAbsolute { x: u32, y: u32 },

    /// Нажатие/отпускание кнопки мыши
    MouseButton {
        button: MouseButton,
        state: ButtonState,
    },

    /// Прокрутка колеса мыши (вертикальная и горизонтальная)
    MouseWheel { delta_y: i16, delta_x: i16 },

    /// Клавиатурное событие с физическим Windows Scan Code
    Keyboard {
        scan_code: u16,
        is_extended: bool,
        state: ButtonState,
    },

    /// Состояние геймпада (XInput совместимое)
    Gamepad(GamepadState),
}

/// Сетевой пакет ввода, привязанный к сессии и защищенный от спуфинга
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputPacket {
    pub session_id: u64,
    pub sequence: u32,
    pub event: InputEvent,
}

impl InputPacket {
    pub fn new(session_id: u64, sequence: u32, event: InputEvent) -> Self {
        Self {
            session_id,
            sequence,
            event,
        }
    }

    /// Сериализация в сетевую датаграмму с префиксом MSG_TYPE_INPUT
    pub fn to_packet(&self) -> Result<Vec<u8>, bincode::Error> {
        let mut buf = Vec::with_capacity(40);
        buf.push(crate::packet::MSG_TYPE_INPUT);
        bincode::serialize_into(&mut buf, self)?;
        Ok(buf)
    }

    /// Строгая десериализация из сетевой датаграммы с префиксом MSG_TYPE_INPUT
    pub fn from_packet(packet: &[u8]) -> Result<Self, bincode::Error> {
        if packet.first() == Some(&crate::packet::MSG_TYPE_INPUT) {
            bincode::deserialize(&packet[1..])
        } else {
            Err(bincode::ErrorKind::Custom(
                "Пакет не содержит префикс MSG_TYPE_INPUT".to_string(),
            )
            .into())
        }
    }
}

impl InputEvent {
    /// Быстрая бинарная сериализация события ввода
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    /// Десериализация бинарного пакета ввода
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }

    /// Упаковка события в сетевой пакет с привязкой к сессии
    pub fn to_packet_with_session(&self, session_id: u64, sequence: u32) -> Result<Vec<u8>, bincode::Error> {
        let packet = InputPacket::new(session_id, sequence, self.clone());
        packet.to_packet()
    }
}

/// Компактная структура состояния геймпада (фиксированный размер)
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable, Serialize, Deserialize)]
pub struct GamepadState {
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub thumb_lx: i16,
    pub thumb_ly: i16,
    pub thumb_rx: i16,
    pub thumb_ry: i16,
}

pub mod gamepad_buttons {
    pub const DPAD_UP: u16 = 0x0001;
    pub const DPAD_DOWN: u16 = 0x0002;
    pub const DPAD_LEFT: u16 = 0x0004;
    pub const DPAD_RIGHT: u16 = 0x0008;
    pub const START: u16 = 0x0010;
    pub const BACK: u16 = 0x0020;
    pub const LEFT_THUMB: u16 = 0x0040;
    pub const RIGHT_THUMB: u16 = 0x0080;
    pub const LEFT_SHOULDER: u16 = 0x0100;
    pub const RIGHT_SHOULDER: u16 = 0x0200;
    pub const A: u16 = 0x1000;
    pub const B: u16 = 0x2000;
    pub const X: u16 = 0x4000;
    pub const Y: u16 = 0x8000;
}
