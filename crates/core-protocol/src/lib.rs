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
    fn test_input_packet_serialization() {
        let event = InputEvent::MouseMoveRelative { dx: -42, dy: 105 };
        let packet = InputPacket::new(0xDEAD_BEEF_CAFE_BABE, 1, event.clone());
        let bytes = packet.to_packet().expect("serialize");
        let decoded = InputPacket::from_packet(&bytes).expect("deserialize");
        assert_eq!(decoded.session_id, 0xDEAD_BEEF_CAFE_BABE);
        assert_eq!(decoded.sequence, 1);
        assert_eq!(decoded.event, event);
    }

    #[test]
    fn test_control_message_serialization() {
        let msg = ControlMessage::RequestKeyframe {
            session_id: 12345,
            reason: KeyframeReason::PacketLoss,
        };
        let bytes = msg.to_packet().expect("serialize");
        let decoded = match ControlMessage::from_packet(&bytes).expect("deserialize") {
            ControlMessage::RequestKeyframe { session_id, reason } => (session_id, reason),
            _ => panic!("wrong variant"),
        };
        assert_eq!(decoded.0, 12345);
        assert_eq!(decoded.1, KeyframeReason::PacketLoss);
    }
}
