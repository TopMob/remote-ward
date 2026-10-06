pub mod control;
pub mod frame;
pub mod input;
pub mod packet;

pub use control::*;
pub use frame::*;
pub use input::*;
pub use packet::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_event_serialization() {
        let event = InputEvent::MouseMoveRelative { dx: -42, dy: 105 };
        let bytes = event.to_bytes().expect("serialize");
        let decoded = InputEvent::from_bytes(&bytes).expect("deserialize");
        assert_eq!(event, decoded);
    }

    #[test]
    fn test_control_message_serialization() {
        let msg = ControlMessage::RequestKeyframe {
            reason: "packet loss detected".into(),
        };
        let bytes = msg.to_bytes().expect("serialize");
        let decoded = match ControlMessage::from_bytes(&bytes).expect("deserialize") {
            ControlMessage::RequestKeyframe { reason } => reason,
            _ => panic!("wrong variant"),
        };
        assert_eq!(decoded, "packet loss detected");
    }
}
