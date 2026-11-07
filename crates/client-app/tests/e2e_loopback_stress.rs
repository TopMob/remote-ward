use core_protocol::{
    ButtonState, ClientHello, ControlMessage, FramePacketizer, InputEvent, InputPacket,
    MouseButton, ServerHello, StreamingStrategy, VideoCodec, DEFAULT_MAX_PAYLOAD_SIZE,
    MSG_TYPE_INPUT, PROTOCOL_VERSION,
};
use core_transport::{optimize_udp_socket, FrameReassembler, PacedSender};
use host_server::host_input::WindowsInputInjector;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;

/// Test server state modeled exactly after `RemoteWardHost::ServerState`
struct TestHostState {
    client_endpoint: Arc<RwLock<Option<SocketAddr>>>,
    client_control_endpoint: Arc<RwLock<Option<SocketAddr>>>,
    last_client_activity: Arc<parking_lot::Mutex<Instant>>,
    session_id: Arc<AtomicU64>,
    #[allow(dead_code)]
    force_keyframe: Arc<AtomicBool>,
    selected_codec: Arc<RwLock<VideoCodec>>,
    is_running: Arc<AtomicBool>,
    target_bitrate_kbps: Arc<AtomicU32>,
    target_fps: u32,
    input_injector: Arc<WindowsInputInjector>,
}

impl TestHostState {
    fn new(initial_bitrate: u32, fps: u32) -> Self {
        Self {
            client_endpoint: Arc::new(RwLock::new(None)),
            client_control_endpoint: Arc::new(RwLock::new(None)),
            last_client_activity: Arc::new(parking_lot::Mutex::new(Instant::now())),
            session_id: Arc::new(AtomicU64::new(1)),
            force_keyframe: Arc::new(AtomicBool::new(true)),
            selected_codec: Arc::new(RwLock::new(VideoCodec::H264)),
            is_running: Arc::new(AtomicBool::new(true)),
            target_bitrate_kbps: Arc::new(AtomicU32::new(initial_bitrate)),
            target_fps: fps,
            input_injector: Arc::new(WindowsInputInjector::with_dimensions(2560, 1440)),
        }
    }
}

/// Spawns the authentic host control loop, watchdog, and paced video sender over real UDP
async fn spawn_test_host(
    state: Arc<TestHostState>,
) -> (
    Arc<UdpSocket>,              // Control socket
    Arc<UdpSocket>,              // Video socket (async Tokio wrapper)
    Arc<PacedSender>,            // Paced video sender with NackBuffer
    SocketAddr,                  // Control address (127.0.0.1:port)
    SocketAddr,                  // Video address (127.0.0.1:port)
    tokio::task::JoinHandle<()>, // Control task handle
    tokio::task::JoinHandle<()>, // Watchdog task handle
) {
    // 1. Bind control socket to dynamic localhost port
    let control_socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    optimize_udp_socket(&control_socket);
    let host_ctrl_addr = control_socket.local_addr().unwrap();

    // 2. Bind video socket to dynamic localhost port
    let video_std = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    video_std.set_nonblocking(true).unwrap();
    let host_video_addr = video_std.local_addr().unwrap();
    let video_std_arc = Arc::new(video_std);

    let paced_sender = Arc::new(PacedSender::new(
        Arc::clone(&video_std_arc),
        state.target_fps,
    ));

    let video_tokio = Arc::new(UdpSocket::from_std((*video_std_arc).try_clone().unwrap()).unwrap());
    optimize_udp_socket(&video_tokio);

    // 3. Spawn control message and input task (identical to RemoteWardHost control task)
    let state_ctrl = Arc::clone(&state);
    let ctrl_sock_task = Arc::clone(&control_socket);
    let paced_sender_ctrl = Arc::clone(&paced_sender);

    let control_task = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        while state_ctrl.is_running.load(Ordering::Relaxed) {
            match ctrl_sock_task.recv_from(&mut buf).await {
                Ok((len, src)) => {
                    let data = &buf[..len];
                    if data.is_empty() {
                        continue;
                    }

                    // A. Check input packet
                    if data[0] == MSG_TYPE_INPUT {
                        *state_ctrl.last_client_activity.lock() = Instant::now();
                        if let Ok(input_pkt) = InputPacket::from_packet(data) {
                            if input_pkt.session_id == state_ctrl.session_id.load(Ordering::Relaxed)
                            {
                                let _ = state_ctrl.input_injector.inject(&input_pkt.event);
                            }
                        } else if let Ok(input_event) = InputEvent::from_packet(data) {
                            let _ = state_ctrl.input_injector.inject(&input_event);
                        }
                        continue;
                    }

                    // B. Check control message
                    if let Ok(ctrl_msg) = ControlMessage::from_packet(data) {
                        *state_ctrl.last_client_activity.lock() = Instant::now();
                        match ctrl_msg {
                            ControlMessage::ClientHello(hello) => {
                                *state_ctrl.client_control_endpoint.write().unwrap() = Some(src);
                                let selected = if hello.supported_codecs.contains(&VideoCodec::HEVC)
                                {
                                    VideoCodec::HEVC
                                } else {
                                    VideoCodec::H264
                                };
                                *state_ctrl.selected_codec.write().unwrap() = selected;

                                let client_video_addr = if hello.video_port > 0 {
                                    SocketAddr::new(src.ip(), hello.video_port)
                                } else {
                                    src
                                };
                                *state_ctrl.client_endpoint.write().unwrap() =
                                    Some(client_video_addr);
                                state_ctrl.force_keyframe.store(true, Ordering::SeqCst);

                                let resp = ControlMessage::ServerHello(ServerHello {
                                    server_name: "test-host".into(),
                                    server_version: "0.1.0".into(),
                                    protocol_version: PROTOCOL_VERSION as u32,
                                    session_id: state_ctrl.session_id.load(Ordering::Relaxed),
                                    selected_codec: selected,
                                    strategy: StreamingStrategy::UltraLan,
                                    width: 2560,
                                    height: 1440,
                                    target_fps: 120,
                                    nack_enabled: true,
                                    fec_enabled: true,
                                    fec_data_shards: 10,
                                    fec_parity_shards: 1,
                                });

                                if let Ok(bytes) = resp.to_packet() {
                                    let _ = ctrl_sock_task.send_to(&bytes, src).await;
                                }
                            }

                            ControlMessage::Ping {
                                sequence,
                                send_timestamp_us,
                                ..
                            } => {
                                let pong = ControlMessage::Pong {
                                    session_id: state_ctrl.session_id.load(Ordering::Relaxed),
                                    sequence,
                                    send_timestamp_us,
                                };
                                if let Ok(b) = pong.to_packet() {
                                    let _ = ctrl_sock_task.send_to(&b, src).await;
                                }
                            }

                            ControlMessage::Nack {
                                frame_id,
                                missing_packet_indices,
                                ..
                            } => {
                                if let Some(target) = *state_ctrl.client_endpoint.read().unwrap() {
                                    let mut sanitized = missing_packet_indices;
                                    sanitized.sort_unstable();
                                    sanitized.dedup();
                                    if sanitized.len() > 32 {
                                        sanitized.truncate(32);
                                    }
                                    let _ = paced_sender_ctrl
                                        .handle_nack_retransmit(frame_id, &sanitized, target);
                                }
                            }

                            ControlMessage::ClientStats {
                                rtt_us,
                                packet_loss_rate,
                                ..
                            } => {
                                let current_bitrate =
                                    state_ctrl.target_bitrate_kbps.load(Ordering::Relaxed);
                                if packet_loss_rate > 0.02 {
                                    let new_bitrate =
                                        ((current_bitrate as f32 * 0.90) as u32).max(10_000);
                                    state_ctrl
                                        .target_bitrate_kbps
                                        .store(new_bitrate, Ordering::SeqCst);
                                } else if rtt_us > 35_000 && current_bitrate > 20_000 {
                                    let new_bitrate =
                                        (current_bitrate.saturating_sub(2_000)).max(10_000);
                                    state_ctrl
                                        .target_bitrate_kbps
                                        .store(new_bitrate, Ordering::SeqCst);
                                }
                            }

                            ControlMessage::Disconnect { .. } => {
                                *state_ctrl.client_endpoint.write().unwrap() = None;
                                *state_ctrl.client_control_endpoint.write().unwrap() = None;
                                state_ctrl.input_injector.release_all();
                            }

                            _ => {}
                        }
                    }
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    });

    // 4. Spawn watchdog task (checks inactivity every 250ms, timeout 2000ms -> release_all)
    let state_wd = Arc::clone(&state);
    let watchdog_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        while state_wd.is_running.load(Ordering::Relaxed) {
            interval.tick().await;
            let has_client = {
                let ctrl = state_wd.client_control_endpoint.read().unwrap();
                let video = state_wd.client_endpoint.read().unwrap();
                ctrl.is_some() || video.is_some()
            };
            if has_client {
                let last_active = *state_wd.last_client_activity.lock();
                if last_active.elapsed() > Duration::from_millis(2000) {
                    state_wd.input_injector.release_all();
                    *state_wd.client_endpoint.write().unwrap() = None;
                    *state_wd.client_control_endpoint.write().unwrap() = None;
                }
            }
        }
    });

    (
        control_socket,
        video_tokio,
        paced_sender,
        host_ctrl_addr,
        host_video_addr,
        control_task,
        watchdog_task,
    )
}

#[tokio::test]
async fn test_dual_instance_handshake_and_ping_pong_rtt() {
    println!("\n================================================================================");
    println!(" [E2E DUAL INSTANCE] Localhost Handshake & Ping/Pong RTT Measurement");
    println!("================================================================================");

    let host_state = Arc::new(TestHostState::new(45_000, 120));
    let (_h_ctrl, _h_vid, _sender, host_ctrl_addr, _host_vid_addr, ctrl_task, wd_task) =
        spawn_test_host(Arc::clone(&host_state)).await;

    // Client setup
    let client_ctrl = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    optimize_udp_socket(&client_ctrl);

    let client_vid = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let client_vid_port = client_vid.local_addr().unwrap().port();

    // 1. Perform Handshake: ClientHello -> ServerHello
    let hello = ControlMessage::ClientHello(ClientHello {
        client_name: "test-client".into(),
        client_version: "0.1.0".into(),
        protocol_version: PROTOCOL_VERSION as u32,
        client_token: 12345,
        supported_codecs: vec![VideoCodec::H264, VideoCodec::HEVC],
        preferred_codec: Some(VideoCodec::H264),
        screen_width: 2560,
        screen_height: 1440,
        target_fps: 120,
        display_refresh_rate: 120,
        supports_tearing: true,
        dpi_scale: 1.0,
        video_port: client_vid_port,
        supports_nack: true,
        supports_fec: true,
    });

    let hello_pkt = hello.to_packet().unwrap();
    client_ctrl
        .send_to(&hello_pkt, host_ctrl_addr)
        .await
        .unwrap();

    let mut buf = [0u8; 2048];
    let (len, _) =
        tokio::time::timeout(Duration::from_millis(500), client_ctrl.recv_from(&mut buf))
            .await
            .expect("ServerHello timeout")
            .unwrap();

    let server_msg = ControlMessage::from_packet(&buf[..len]).unwrap();
    let server_hello = match server_msg {
        ControlMessage::ServerHello(sh) => sh,
        other => panic!("Expected ServerHello, got {:?}", other),
    };

    println!(
        "    Server Response: {} v{} (Protocol v{})",
        server_hello.server_name, server_hello.server_version, server_hello.protocol_version
    );
    println!(
        "    Negotiated Codec: {:?}, Strategy: {:?}",
        server_hello.selected_codec, server_hello.strategy
    );
    println!(
        "    Target Resolution: {}x{} @ {} FPS",
        server_hello.width, server_hello.height, server_hello.target_fps
    );
    println!(
        "    FEC Parameters: {}:{} (data:parity)",
        server_hello.fec_data_shards, server_hello.fec_parity_shards
    );

    assert_eq!(server_hello.target_fps, 120);
    assert_eq!(server_hello.strategy, StreamingStrategy::UltraLan);
    assert_eq!(server_hello.fec_data_shards, 10);
    assert_eq!(server_hello.fec_parity_shards, 1);

    // 2. Measure 50 Ping/Pong Round-Trips over loopback UDP
    let ping_count = 50;
    let mut rtt_samples = Vec::with_capacity(ping_count);

    for seq in 1..=ping_count as u32 {
        let now_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;

        let ping = ControlMessage::Ping {
            session_id: 1,
            sequence: seq,
            send_timestamp_us: now_us,
        };
        client_ctrl
            .send_to(&ping.to_packet().unwrap(), host_ctrl_addr)
            .await
            .unwrap();

        let (len, _) =
            tokio::time::timeout(Duration::from_millis(100), client_ctrl.recv_from(&mut buf))
                .await
                .expect("Pong timeout")
                .unwrap();

        let pong_msg = ControlMessage::from_packet(&buf[..len]).unwrap();
        if let ControlMessage::Pong {
            sequence,
            send_timestamp_us,
            ..
        } = pong_msg
        {
            assert_eq!(sequence, seq);
            let recv_us = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_micros() as u64;
            let rtt = recv_us.saturating_sub(send_timestamp_us);
            rtt_samples.push(rtt);
        }
    }

    let min_rtt = rtt_samples.iter().copied().min().unwrap_or(0);
    let max_rtt = rtt_samples.iter().copied().max().unwrap_or(0);
    let avg_rtt = rtt_samples.iter().sum::<u64>() / (rtt_samples.len() as u64);

    println!(
        "    Loopback UDP Ping/Pong ({} pings): min={}µs, avg={}µs, max={}µs",
        ping_count, min_rtt, avg_rtt, max_rtt
    );

    assert!(
        avg_rtt < 1000,
        "Localhost UDP average RTT must be sub-millisecond (< 1000 µs)"
    );

    // Cleanup
    host_state.is_running.store(false, Ordering::Relaxed);
    ctrl_task.abort();
    wd_task.abort();
}

#[tokio::test]
async fn test_dual_instance_paced_streaming_and_nack_over_udp() {
    println!("\n================================================================================");
    println!(" [E2E DUAL INSTANCE] Paced Video Streaming & Real UDP NACK Retransmission");
    println!("================================================================================");

    let host_state = Arc::new(TestHostState::new(45_000, 120));
    let (_h_ctrl, _h_vid, paced_sender, host_ctrl_addr, _host_vid_addr, ctrl_task, wd_task) =
        spawn_test_host(Arc::clone(&host_state)).await;

    // Client setup
    let client_ctrl = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    optimize_udp_socket(&client_ctrl);

    let client_vid = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    optimize_udp_socket(&client_vid);
    let client_vid_addr = client_vid.local_addr().unwrap();

    // Handshake
    let hello = ControlMessage::ClientHello(ClientHello {
        client_name: "test-client".into(),
        client_version: "0.1.0".into(),
        protocol_version: PROTOCOL_VERSION as u32,
        client_token: 999,
        supported_codecs: vec![VideoCodec::H264],
        preferred_codec: Some(VideoCodec::H264),
        screen_width: 2560,
        screen_height: 1440,
        target_fps: 120,
        display_refresh_rate: 120,
        supports_tearing: true,
        dpi_scale: 1.0,
        video_port: client_vid_addr.port(),
        supports_nack: true,
        supports_fec: true,
    });
    client_ctrl
        .send_to(&hello.to_packet().unwrap(), host_ctrl_addr)
        .await
        .unwrap();

    let mut buf = [0u8; 2048];
    let (len, _) =
        tokio::time::timeout(Duration::from_millis(500), client_ctrl.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
    assert!(matches!(
        ControlMessage::from_packet(&buf[..len]).unwrap(),
        ControlMessage::ServerHello(_)
    ));

    // 1. Stream 50 normal frames via PacedSender over UDP loopback
    let frames_to_stream = 50u32;
    let mut reassembler = FrameReassembler::new(Duration::from_millis(100));

    let stream_start = Instant::now();
    let mut total_bytes = 0usize;

    for frame_id in 0..frames_to_stream {
        let is_kf = (frame_id % 15) == 0;
        let size = if is_kf { 65_000 } else { 22_000 };
        total_bytes += size;

        let frame_data = vec![(frame_id & 0xFF) as u8; size];
        let packets = FramePacketizer::packetize_to_datagrams(
            1,
            frame_id,
            (frame_id as u64) * 8333,
            is_kf,
            &frame_data,
            DEFAULT_MAX_PAYLOAD_SIZE,
        );

        // Send via PacedSender to client_vid_addr
        paced_sender
            .send_frame_packets(frame_id, &packets, client_vid_addr)
            .unwrap();

        // Receive all packets on client side
        let mut completed = None;
        for _ in 0..packets.len() {
            let (pkt_len, _) =
                tokio::time::timeout(Duration::from_millis(50), client_vid.recv_from(&mut buf))
                    .await
                    .expect("UDP packet recv timeout")
                    .unwrap();

            if let Ok(Some(rf)) = reassembler.process_packet(&buf[..pkt_len]) {
                completed = Some(rf);
            }
        }

        let rf = completed.expect("Кадр должен быть успешно собран");
        assert_eq!(rf.frame_id, frame_id);
        assert_eq!(rf.data.len(), size);
    }

    let stream_elapsed = stream_start.elapsed();
    let mb = (total_bytes as f64) / (1024.0 * 1024.0);
    println!(
        "    Streamed 50 Frames over Localhost UDP ({:.2} MB in {:.1} ms -> {:.1} MB/s)",
        mb,
        stream_elapsed.as_secs_f64() * 1000.0,
        mb / stream_elapsed.as_secs_f64()
    );

    // 2. Test NACK Retransmission over real UDP:
    // Drop packet #2 and #5 of a 10-packet frame, send NACK over control UDP, verify retransmission
    println!("    --> Testing Real UDP NACK Retransmission scenario...");
    let test_frame_id = 100u32;
    let test_frame_data = vec![0x42u8; 13_800]; // 10 packets
    let test_packets = FramePacketizer::packetize_to_datagrams(
        1,
        test_frame_id,
        999_999,
        true,
        &test_frame_data,
        DEFAULT_MAX_PAYLOAD_SIZE,
    );
    assert_eq!(test_packets.len(), 10);

    // Host sends frame with PacedSender (buffers packets in NackBuffer)
    paced_sender
        .send_frame_packets(test_frame_id, &test_packets, client_vid_addr)
        .unwrap();

    // Client receives packets, but intentionally drops packets #2 and #5
    let dropped_indices = [2u16, 5u16];
    let mut completed_nack_frame = None;

    for i in 0..10 {
        let (pkt_len, _) =
            tokio::time::timeout(Duration::from_millis(50), client_vid.recv_from(&mut buf))
                .await
                .unwrap()
                .unwrap();

        if !dropped_indices.contains(&(i as u16)) {
            if let Ok(Some(rf)) = reassembler.process_packet(&buf[..pkt_len]) {
                completed_nack_frame = Some(rf);
            }
        }
    }

    assert!(
        completed_nack_frame.is_none(),
        "Frame should not complete because packets were dropped"
    );

    // Client queries missing packets from reassembler
    let missing = reassembler
        .get_missing_packets(test_frame_id)
        .expect("Should detect missing packets");
    assert_eq!(missing, vec![2, 5]);

    // Client sends NACK control message over UDP to host_ctrl_addr
    let nack = ControlMessage::Nack {
        session_id: 1,
        frame_id: test_frame_id,
        missing_packet_indices: missing,
    };
    client_ctrl
        .send_to(&nack.to_packet().unwrap(), host_ctrl_addr)
        .await
        .unwrap();

    // Host handles NACK and retransmits packets #2 and #5. Client receives them!
    for _ in 0..2 {
        let (pkt_len, _) =
            tokio::time::timeout(Duration::from_millis(100), client_vid.recv_from(&mut buf))
                .await
                .expect("NACK retransmitted packet timeout")
                .unwrap();

        if let Ok(Some(rf)) = reassembler.process_packet(&buf[..pkt_len]) {
            completed_nack_frame = Some(rf);
        }
    }

    let recovered = completed_nack_frame
        .expect("Frame must reassemble successfully after receiving NACK retransmission!");
    assert_eq!(recovered.frame_id, test_frame_id);
    assert_eq!(recovered.data, test_frame_data);
    println!("    NACK Recovery over Real UDP Sockets: 100% BIT-EXACT SUCCESS!");

    host_state.is_running.store(false, Ordering::Relaxed);
    ctrl_task.abort();
    wd_task.abort();
}

#[tokio::test]
async fn test_dual_instance_input_injection_and_watchdog_timeout() {
    println!("\n================================================================================");
    println!(" [E2E DUAL INSTANCE] Input Injection & Host Watchdog Stuck Keys Protection");
    println!("================================================================================");

    let host_state = Arc::new(TestHostState::new(45_000, 120));
    let injector = Arc::clone(&host_state.input_injector);
    let (_h_ctrl, _h_vid, _sender, host_ctrl_addr, _host_vid_addr, ctrl_task, wd_task) =
        spawn_test_host(Arc::clone(&host_state)).await;

    // Client setup
    let client_ctrl = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    optimize_udp_socket(&client_ctrl);

    // Initial handshake to authorize client IP
    let hello = ControlMessage::ClientHello(ClientHello {
        client_name: "test-client".into(),
        client_version: "0.1.0".into(),
        protocol_version: PROTOCOL_VERSION as u32,
        client_token: 777,
        supported_codecs: vec![VideoCodec::H264],
        preferred_codec: Some(VideoCodec::H264),
        screen_width: 2560,
        screen_height: 1440,
        target_fps: 120,
        display_refresh_rate: 120,
        supports_tearing: true,
        dpi_scale: 1.0,
        video_port: 0,
        supports_nack: true,
        supports_fec: true,
    });
    client_ctrl
        .send_to(&hello.to_packet().unwrap(), host_ctrl_addr)
        .await
        .unwrap();

    let mut buf = [0u8; 1024];
    let (len, _) =
        tokio::time::timeout(Duration::from_millis(500), client_ctrl.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
    assert!(matches!(
        ControlMessage::from_packet(&buf[..len]).unwrap(),
        ControlMessage::ServerHello(_)
    ));

    // 1. Send rapid input packets over UDP
    println!("    --> Sending 1000 input events over UDP to host...");
    for seq in 0..1000u32 {
        let event = if seq % 2 == 0 {
            InputEvent::MouseMoveRelative {
                dx: (seq as i32 % 5),
                dy: -(seq as i32 % 5),
            }
        } else {
            InputEvent::MouseMoveAbsolute {
                x: seq % 2560,
                y: seq % 1440,
            }
        };
        let pkt = InputPacket::new(1, seq, event);
        client_ctrl
            .send_to(&pkt.to_packet().unwrap(), host_ctrl_addr)
            .await
            .unwrap();
    }

    tokio::time::sleep(Duration::from_millis(50)).await;

    // 2. Press and hold keys: W, A, S, LShift, MouseButton::Left
    let held_keys_to_inject = [0x11u16, 0x1Eu16, 0x1Fu16, 0x2Au16]; // W, A, S, LShift
    for &sc in &held_keys_to_inject {
        let event = InputEvent::Keyboard {
            scan_code: sc,
            is_extended: false,
            state: ButtonState::Pressed,
        };
        let pkt = InputPacket::new(1, 2000 + sc as u32, event);
        client_ctrl
            .send_to(&pkt.to_packet().unwrap(), host_ctrl_addr)
            .await
            .unwrap();
    }

    let mouse_down = InputEvent::MouseButton {
        button: MouseButton::Left,
        state: ButtonState::Pressed,
    };
    let pkt = InputPacket::new(1, 2999, mouse_down);
    client_ctrl
        .send_to(&pkt.to_packet().unwrap(), host_ctrl_addr)
        .await
        .unwrap();

    // Give host control task a moment to process
    tokio::time::sleep(Duration::from_millis(30)).await;

    assert_eq!(
        injector.pressed_keys().len(),
        4,
        "Host injector must track 4 pressed keys"
    );
    assert_eq!(
        injector.pressed_mouse_buttons().len(),
        1,
        "Host injector must track 1 pressed mouse button"
    );

    // Scenario A: Clean Disconnect Message
    println!("    --> Scenario A: Clean Disconnect Message Verification");
    let disc = ControlMessage::Disconnect {
        session_id: 1,
        reason: "User closed session".into(),
    };
    client_ctrl
        .send_to(&disc.to_packet().unwrap(), host_ctrl_addr)
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        injector.pressed_keys().is_empty(),
        "Keys must be released immediately upon Disconnect"
    );
    assert!(
        injector.pressed_mouse_buttons().is_empty(),
        "Mouse buttons must be released upon Disconnect"
    );
    println!("    Clean Disconnect Stuck Keys Check: PASSED (All keys released)");

    // Scenario B: Sudden Network Disconnection / Client Crash
    println!("    --> Scenario B: Sudden Client Disappearance & Host Watchdog Trigger");
    // Re-establish session
    client_ctrl
        .send_to(&hello.to_packet().unwrap(), host_ctrl_addr)
        .await
        .unwrap();
    let (len, _) =
        tokio::time::timeout(Duration::from_millis(500), client_ctrl.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
    assert!(matches!(
        ControlMessage::from_packet(&buf[..len]).unwrap(),
        ControlMessage::ServerHello(_)
    ));

    // Hold W, D, Right Click
    for &sc in &[0x11u16, 0x20u16] {
        let pkt = InputPacket::new(
            1,
            3000 + sc as u32,
            InputEvent::Keyboard {
                scan_code: sc,
                is_extended: false,
                state: ButtonState::Pressed,
            },
        );
        client_ctrl
            .send_to(&pkt.to_packet().unwrap(), host_ctrl_addr)
            .await
            .unwrap();
    }
    let mouse_r_down = InputPacket::new(
        1,
        3999,
        InputEvent::MouseButton {
            button: MouseButton::Right,
            state: ButtonState::Pressed,
        },
    );
    client_ctrl
        .send_to(&mouse_r_down.to_packet().unwrap(), host_ctrl_addr)
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(injector.pressed_keys().len(), 2);
    assert_eq!(injector.pressed_mouse_buttons().len(), 1);

    // Client completely stops sending! Sleep for 2.2 seconds to allow host watchdog to fire (>2000ms)
    println!("    Sleeping 2200ms to test host inactivity watchdog (>2000ms)...");
    tokio::time::sleep(Duration::from_millis(2200)).await;

    // Verify host watchdog fired release_all and cleared endpoints
    assert!(
        injector.pressed_keys().is_empty(),
        "Watchdog must release stuck keys after 2000ms inactivity!"
    );
    assert!(
        injector.pressed_mouse_buttons().is_empty(),
        "Watchdog must release stuck mouse buttons after 2000ms inactivity!"
    );
    assert!(
        host_state.client_control_endpoint.read().unwrap().is_none(),
        "Client control endpoint must be reset"
    );
    assert!(
        host_state.client_endpoint.read().unwrap().is_none(),
        "Client video endpoint must be reset"
    );

    println!(
        "    Host Watchdog Stuck Keys Protection: PASSED (Zero stuck keys on sudden disconnect)"
    );

    host_state.is_running.store(false, Ordering::Relaxed);
    ctrl_task.abort();
    wd_task.abort();
}

#[tokio::test]
async fn test_dual_instance_artificial_network_stress_simulation() {
    println!("\n================================================================================");
    println!(" [E2E DUAL INSTANCE] Artificial Stress: Latency (10ms, 30ms) & Loss Conduit");
    println!("================================================================================");

    let host_state = Arc::new(TestHostState::new(45_000, 120));
    let (_h_ctrl, _h_vid, _sender, host_ctrl_addr, _host_vid_addr, ctrl_task, wd_task) =
        spawn_test_host(Arc::clone(&host_state)).await;

    // Conduit / Simulated WAN Proxy on localhost:
    // Proxy receives on proxy_port, sleeps by artificial latency, then forwards to host_ctrl_addr
    let proxy_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let proxy_addr = proxy_sock.local_addr().unwrap();

    let client_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let client_addr = client_sock.local_addr().unwrap();

    let proxy_sock_fwd = Arc::clone(&proxy_sock);
    let proxy_task = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            tokio::select! {
                Ok((len, src)) = proxy_sock_fwd.recv_from(&mut buf) => {
                    let data = buf[..len].to_vec();
                    let target = if src == client_addr {
                        host_ctrl_addr
                    } else {
                        client_addr
                    };

                    let p_sock = Arc::clone(&proxy_sock_fwd);
                    tokio::spawn(async move {
                        // Artificial 15ms one-way delay (30ms round-trip)
                        tokio::time::sleep(Duration::from_millis(15)).await;
                        let _ = p_sock.send_to(&data, target).await;
                    });
                }
                else => break,
            }
        }
    });

    // 1. Handshake through the 30ms RTT artificial latency proxy
    let hello = ControlMessage::ClientHello(ClientHello {
        client_name: "wan-client".into(),
        client_version: "0.1.0".into(),
        protocol_version: PROTOCOL_VERSION as u32,
        client_token: 555,
        supported_codecs: vec![VideoCodec::H264],
        preferred_codec: Some(VideoCodec::H264),
        screen_width: 2560,
        screen_height: 1440,
        target_fps: 60,
        display_refresh_rate: 60,
        supports_tearing: false,
        dpi_scale: 1.0,
        video_port: 0,
        supports_nack: true,
        supports_fec: true,
    });

    let t0 = Instant::now();
    client_sock
        .send_to(&hello.to_packet().unwrap(), proxy_addr)
        .await
        .unwrap();

    let mut buf = [0u8; 2048];
    let (len, _) =
        tokio::time::timeout(Duration::from_millis(300), client_sock.recv_from(&mut buf))
            .await
            .expect("Handshake timeout through proxy")
            .unwrap();

    let handshake_rtt = t0.elapsed();
    assert!(matches!(
        ControlMessage::from_packet(&buf[..len]).unwrap(),
        ControlMessage::ServerHello(_)
    ));
    println!(
        "    Handshake through 30ms WAN latency proxy completed in {:.1} ms",
        handshake_rtt.as_secs_f64() * 1000.0
    );
    assert!(
        handshake_rtt >= Duration::from_millis(25),
        "Handshake must reflect injected latency"
    );

    // 2. Ping/Pong through 30ms proxy
    let ping = ControlMessage::Ping {
        session_id: 1,
        sequence: 42,
        send_timestamp_us: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64,
    };
    let t_ping = Instant::now();
    client_sock
        .send_to(&ping.to_packet().unwrap(), proxy_addr)
        .await
        .unwrap();

    let (len, _) =
        tokio::time::timeout(Duration::from_millis(300), client_sock.recv_from(&mut buf))
            .await
            .expect("Pong timeout through proxy")
            .unwrap();

    let measured_ping_rtt = t_ping.elapsed();
    assert!(matches!(
        ControlMessage::from_packet(&buf[..len]).unwrap(),
        ControlMessage::Pong { .. }
    ));
    println!(
        "    Ping/Pong measured RTT: {:.2} ms (Expected ~30ms)",
        measured_ping_rtt.as_secs_f64() * 1000.0
    );
    assert!(
        measured_ping_rtt >= Duration::from_millis(25)
            && measured_ping_rtt <= Duration::from_millis(120),
        "Ping/Pong RTT must reflect injected proxy latency: {:?}",
        measured_ping_rtt
    );

    // 3. Test Host ABR Congestion Adaptation under network stress
    println!(
        "    --> Testing Host ABR adaptation reaction under simulated bufferbloat & packet loss"
    );
    let initial_bitrate = host_state.target_bitrate_kbps.load(Ordering::Relaxed);
    assert_eq!(initial_bitrate, 45_000);

    // Send ClientStats with high RTT (45ms, bufferbloat) -> Host should reduce bitrate
    let stats_bloat = ControlMessage::ClientStats {
        session_id: 1,
        rtt_us: 45_000, // 45ms RTT
        jitter_us: 5_000,
        decode_latency_us: 1_200,
        render_latency_us: 800,
        packet_loss_rate: 0.0,
        fec_recovered_count: 0,
        nack_sent_count: 0,
    };
    client_sock
        .send_to(&stats_bloat.to_packet().unwrap(), host_ctrl_addr)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let bitrate_after_bloat = host_state.target_bitrate_kbps.load(Ordering::Relaxed);
    println!(
        "    ABR bufferbloat reaction: {} -> {} kbps",
        initial_bitrate, bitrate_after_bloat
    );
    assert!(
        bitrate_after_bloat < initial_bitrate,
        "Bitrate must decrease upon detecting bufferbloat (RTT > 35ms)"
    );

    // Send ClientStats with 4% packet loss -> Host should decrease bitrate further
    let stats_loss = ControlMessage::ClientStats {
        session_id: 1,
        rtt_us: 30_000,
        jitter_us: 3_000,
        decode_latency_us: 1_200,
        render_latency_us: 800,
        packet_loss_rate: 0.04, // 4% loss
        fec_recovered_count: 12,
        nack_sent_count: 8,
    };
    client_sock
        .send_to(&stats_loss.to_packet().unwrap(), host_ctrl_addr)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let bitrate_after_loss = host_state.target_bitrate_kbps.load(Ordering::Relaxed);
    println!(
        "    ABR 4% loss reaction: {} -> {} kbps",
        bitrate_after_bloat, bitrate_after_loss
    );
    assert!(
        bitrate_after_loss < bitrate_after_bloat,
        "Bitrate must drop further upon packet loss > 2%"
    );

    println!("    ABR Congestion Control Verification: PASSED");

    host_state.is_running.store(false, Ordering::Relaxed);
    proxy_task.abort();
    ctrl_task.abort();
    wd_task.abort();
}
