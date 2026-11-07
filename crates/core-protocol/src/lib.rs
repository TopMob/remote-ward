pub mod control;
pub mod discovery;
pub mod frame;
pub mod input;
pub mod packet;

pub use control::*;
pub use discovery::*;
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

        // Проверка сериализации в срез памяти без аллокаций (Zero-Allocation)
        let mut buf = [0u8; 64];
        let len = event.write_to_slice(&mut buf).expect("write_to_slice");
        let decoded_from_slice = InputEvent::from_packet(&buf[..len]).expect("deserialize slice");
        assert_eq!(event, decoded_from_slice);
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

    #[test]
    fn test_switch_monitor_messages() {
        let switch_req = ControlMessage::SwitchMonitor {
            session_id: 42,
            monitor_index: 1,
        };
        let pkt = switch_req.to_packet().expect("serialize");
        let decoded = ControlMessage::from_packet(&pkt).expect("deserialize");
        match decoded {
            ControlMessage::SwitchMonitor {
                session_id,
                monitor_index,
            } => {
                assert_eq!(session_id, 42);
                assert_eq!(monitor_index, 1);
            }
            _ => panic!("Expected SwitchMonitor"),
        }

        let switch_ack = ControlMessage::SwitchMonitorAck {
            session_id: 42,
            monitor_index: 1,
            width: 2560,
            height: 1440,
        };
        let ack_pkt = switch_ack.to_packet().expect("serialize ack");
        let decoded_ack = ControlMessage::from_packet(&ack_pkt).expect("deserialize ack");
        match decoded_ack {
            ControlMessage::SwitchMonitorAck {
                session_id,
                monitor_index,
                width,
                height,
            } => {
                assert_eq!(session_id, 42);
                assert_eq!(monitor_index, 1);
                assert_eq!(width, 2560);
                assert_eq!(height, 1440);
            }
            _ => panic!("Expected SwitchMonitorAck"),
        }
    }

    #[test]
    fn test_client_server_hello_roundtrip() {
        let server_hello = ControlMessage::ServerHello(ServerHello {
            server_name: "test-server".into(),
            server_version: "0.1.0".into(),
            protocol_version: 1,
            session_id: 100,
            selected_codec: VideoCodec::H264,
            strategy: StreamingStrategy::UltraLan,
            width: 1920,
            height: 1080,
            target_fps: 120,
            nack_enabled: true,
            fec_enabled: true,
            fec_data_shards: 10,
            fec_parity_shards: 1,
        });

        let pkt = server_hello.to_packet().expect("serialize");
        let decoded = ControlMessage::from_packet(&pkt).expect("deserialize");
        match decoded {
            ControlMessage::ServerHello(h) => {
                assert_eq!(h.server_name, "test-server");
                assert_eq!(h.strategy, StreamingStrategy::UltraLan);
                assert_eq!(h.target_fps, 120);
            }
            _ => panic!("Expected ServerHello"),
        }
    }

    #[test]
    fn test_calibration_and_cursor_messages() {
        let cal = ControlMessage::CalibrationReport {
            session_id: 1,
            min_rtt_us: 1500,
            avg_rtt_us: 2500,
            jitter_us: 400,
            loss_rate: 0.0,
            selected_bitrate_kbps: 30000,
        };
        let cal_pkt = cal.to_packet().expect("serialize cal");
        let dec_cal = ControlMessage::from_packet(&cal_pkt).expect("deserialize cal");
        match dec_cal {
            ControlMessage::CalibrationReport {
                min_rtt_us,
                selected_bitrate_kbps,
                ..
            } => {
                assert_eq!(min_rtt_us, 1500);
                assert_eq!(selected_bitrate_kbps, 30000);
            }
            _ => panic!("Expected CalibrationReport"),
        }

        let cursor = ControlMessage::SetLocalCursor {
            session_id: 1,
            enabled: true,
        };
        let cursor_pkt = cursor.to_packet().expect("serialize cursor");
        let dec_cursor = ControlMessage::from_packet(&cursor_pkt).expect("deserialize cursor");
        match dec_cursor {
            ControlMessage::SetLocalCursor { enabled, .. } => {
                assert!(enabled);
            }
            _ => panic!("Expected SetLocalCursor"),
        }
    }

    #[test]
    fn test_malicious_bincode_oom_protection() {
        // Пакет с префиксом MSG_TYPE_CONTROL и огромной длиной вектора (0xFFFF_FFFF_FFFF_FFFF)
        let mut evil = vec![crate::packet::MSG_TYPE_CONTROL];
        evil.extend_from_slice(&[3, 0, 0, 0]); // дискриминант enum (Nack = 3)
        evil.extend_from_slice(&[0xEF, 0xBE, 0xAD, 0xDE, 0, 0, 0, 0]); // session_id
        evil.extend_from_slice(&[1, 0, 0, 0]); // frame_id
        evil.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]); // длина вектора 18 эксабайт
        assert!(ControlMessage::from_packet(&evil).is_err());
    }
}
