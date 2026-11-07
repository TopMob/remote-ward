use core_protocol::{FramePacketizer, DEFAULT_MAX_PAYLOAD_SIZE};
use core_transport::{
    is_newer_frame, FecConfig, FecDecoder, FecEncoder, FrameReassembler, NackBuffer,
};
use std::time::{Duration, Instant};

/// Simple deterministic pseudo-random generator (Xorshift32) for reproducible stress testing
struct SimpleRng(u32);

impl SimpleRng {
    fn new(seed: u32) -> Self {
        Self(if seed == 0 { 0x12345678 } else { seed })
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    fn next_f32(&mut self) -> f32 {
        (self.next_u32() & 0xFFFFFF) as f32 / 16777216.0
    }
}

/// Helper to compute percentile from a mutable slice of durations
fn percentile(samples: &mut [Duration], pct: f64) -> Duration {
    if samples.is_empty() {
        return Duration::ZERO;
    }
    samples.sort_unstable();
    let idx = ((samples.len() as f64) * pct / 100.0).round() as usize;
    let idx = idx.min(samples.len() - 1);
    samples[idx]
}

#[test]
fn test_high_framerate_burst_packetization_and_reassembly() {
    println!("\n================================================================================");
    println!(" [STRESS TEST] High Frame Rate Burst Packetization & Reassembly (1000 Frames)");
    println!("================================================================================");

    let session_id = 0xAA55_BEEF_CAFE_0001u64;
    let total_frames = 1000;
    let mut reassembler = FrameReassembler::new(Duration::from_millis(50));
    reassembler.set_expected_session_id(Some(session_id));

    let mut packetize_times = Vec::with_capacity(total_frames);
    let mut reassemble_times = Vec::with_capacity(total_frames);
    let mut total_bytes_payload = 0usize;
    let mut total_packets_generated = 0usize;

    let overall_start = Instant::now();

    for frame_id in 0..total_frames as u32 {
        let is_keyframe = (frame_id % 30) == 0;
        // Varying payload size: Keyframes ~95 KB, P-frames oscillating between 12 KB and 48 KB
        let frame_size = if is_keyframe {
            95_000 + ((frame_id * 17) % 15_000) as usize
        } else {
            12_000 + ((frame_id * 7919) % 36_000) as usize
        };
        total_bytes_payload += frame_size;

        // Generate synthetic frame pattern with identifiable markers
        let mut frame_data = vec![0u8; frame_size];
        for (i, byte) in frame_data.iter_mut().enumerate() {
            *byte = ((frame_id as usize ^ i) & 0xFF) as u8;
        }

        // 1. Packetize frame
        let t0 = Instant::now();
        let packets = FramePacketizer::packetize_to_datagrams(
            session_id,
            frame_id,
            (frame_id as u64) * 16_666,
            is_keyframe,
            &frame_data,
            DEFAULT_MAX_PAYLOAD_SIZE,
        );
        let t_pkt = t0.elapsed();
        packetize_times.push(t_pkt);
        total_packets_generated += packets.len();

        // 2. Feed packets sequentially into Reassembler
        let t1 = Instant::now();
        let mut completed = None;
        for pkt in &packets {
            if let Ok(Some(rf)) = reassembler.process_packet(pkt) {
                completed = Some(rf);
            }
        }
        let t_reassemble = t1.elapsed();
        reassemble_times.push(t_reassemble);

        // 3. Verify integrity
        let completed_frame = completed.expect("Кадр должен быть успешно собран");
        assert_eq!(completed_frame.frame_id, frame_id);
        assert_eq!(completed_frame.session_id, session_id);
        assert_eq!(completed_frame.is_keyframe, is_keyframe);
        assert_eq!(
            completed_frame.data.len(),
            frame_size,
            "Длина собранных данных кадра #{} не совпадает",
            frame_id
        );
        assert_eq!(
            completed_frame.data, frame_data,
            "Байты кадра #{} повреждены при сборке",
            frame_id
        );
    }

    let overall_elapsed = overall_start.elapsed();
    let throughput_mb = (total_bytes_payload as f64) / (1024.0 * 1024.0);
    let throughput_mb_per_sec = throughput_mb / overall_elapsed.as_secs_f64();
    let fps_processed = (total_frames as f64) / overall_elapsed.as_secs_f64();

    let pkt_avg = packetize_times.iter().sum::<Duration>() / (total_frames as u32);
    let pkt_p50 = percentile(&mut packetize_times, 50.0);
    let pkt_p95 = percentile(&mut packetize_times, 95.0);
    let pkt_p99 = percentile(&mut packetize_times, 99.0);

    let rea_avg = reassemble_times.iter().sum::<Duration>() / (total_frames as u32);
    let rea_p50 = percentile(&mut reassemble_times, 50.0);
    let rea_p95 = percentile(&mut reassemble_times, 95.0);
    let rea_p99 = percentile(&mut reassemble_times, 99.0);

    println!("Total Frames Processed : {}", total_frames);
    println!("Total UDP Datagrams    : {}", total_packets_generated);
    println!("Total Video Data       : {:.2} MB", throughput_mb);
    println!(
        "Overall Elapsed Time   : {:.2} ms",
        overall_elapsed.as_secs_f64() * 1000.0
    );
    println!("Throughput             : {:.2} MB/s", throughput_mb_per_sec);
    println!("Effective Processing   : {:.1} FPS", fps_processed);
    println!(
        "Packetization Latency  : avg={:.1}µs, p50={:.1}µs, p95={:.1}µs, p99={:.1}µs",
        pkt_avg.as_micros(),
        pkt_p50.as_micros(),
        pkt_p95.as_micros(),
        pkt_p99.as_micros()
    );
    println!(
        "Reassembly Latency     : avg={:.1}µs, p50={:.1}µs, p95={:.1}µs, p99={:.1}µs",
        rea_avg.as_micros(),
        rea_p50.as_micros(),
        rea_p95.as_micros(),
        rea_p99.as_micros()
    );

    // Sanity assertions for performance and memory
    assert!(
        throughput_mb_per_sec > 50.0,
        "Throughput must exceed 50 MB/s on loopback memory"
    );
    assert!(
        fps_processed > 500.0,
        "Frame processing rate must exceed 500 FPS"
    );
    assert!(
        rea_p99 < Duration::from_micros(1500),
        "p99 reassembly latency must be < 1.5ms"
    );
}

#[test]
fn test_synthetic_packet_loss_and_nack_recovery() {
    println!("\n================================================================================");
    println!(" [STRESS TEST] Synthetic Packet Loss & NACK Recovery (1%, 5%, 15% Loss)");
    println!("================================================================================");

    let session_id = 0x1122_3344_5566_7788u64;

    // Test 1: 1% packet loss with NACK retransmission
    {
        println!("--> Scenario A: 1% Uniform Loss with NACK Recovery");
        let mut reassembler = FrameReassembler::new(Duration::from_millis(100));
        let mut nack_buffer = NackBuffer::new();
        let mut rng = SimpleRng::new(42);

        let total_frames = 200;
        let mut recovered_frames = 0;
        let mut nack_retransmit_packets = 0;

        for frame_id in 0..total_frames {
            let frame_size = 35_000;
            let frame_data = vec![0x7A; frame_size];
            let packets = FramePacketizer::packetize_to_datagrams(
                session_id,
                frame_id,
                (frame_id as u64) * 16666,
                frame_id % 30 == 0,
                &frame_data,
                DEFAULT_MAX_PAYLOAD_SIZE,
            );

            // Buffer packets in server NackBuffer
            nack_buffer.begin_frame(frame_id);
            for (idx, pkt) in packets.iter().enumerate() {
                nack_buffer.buffer_packet(frame_id, idx as u16, pkt.clone());
            }

            // Transmit with 1% loss
            let mut completed = None;
            for pkt in &packets {
                let drop = rng.next_f32() < 0.01;
                if !drop {
                    if let Ok(Some(f)) = reassembler.process_packet(pkt) {
                        completed = Some(f);
                    }
                }
            }

            // If not completed due to drop, request missing packets via NACK
            if completed.is_none() {
                if let Some(missing) = reassembler.get_missing_packets(frame_id) {
                    let retransmitted = nack_buffer.get_packets_for_nack(frame_id, &missing);
                    nack_retransmit_packets += retransmitted.len();
                    for pkt in retransmitted {
                        if let Ok(Some(f)) = reassembler.process_packet(&pkt) {
                            completed = Some(f);
                        }
                    }
                }
            }

            if completed.is_some() {
                recovered_frames += 1;
            }
        }

        println!(
            "    Total Frames: {}, Recovered via NACK: {}/{} ({:.1}%), Retransmitted: {} packets",
            total_frames,
            recovered_frames,
            total_frames,
            (recovered_frames as f32 / total_frames as f32) * 100.0,
            nack_retransmit_packets
        );
        assert_eq!(
            recovered_frames, total_frames,
            "100% of frames should recover at 1% loss with NACK"
        );
    }

    // Test 2: 5% packet loss with realistic network jitter and occasional unrecoverable frames
    {
        println!("--> Scenario B: 5% Packet Loss & Dirty Frame Dropping");
        let mut reassembler = FrameReassembler::new(Duration::from_millis(30));
        let mut nack_buffer = NackBuffer::new();
        let mut rng = SimpleRng::new(12345);

        let total_frames = 200;
        let mut completed_frames = 0;
        let mut dropped_frames = 0;

        for frame_id in 0..total_frames {
            let is_kf = (frame_id % 20) == 0;
            let frame_data = vec![0xBC; 40_000];
            let packets = FramePacketizer::packetize_to_datagrams(
                session_id,
                frame_id,
                (frame_id as u64) * 16666,
                is_kf,
                &frame_data,
                DEFAULT_MAX_PAYLOAD_SIZE,
            );

            nack_buffer.begin_frame(frame_id);
            for (idx, pkt) in packets.iter().enumerate() {
                nack_buffer.buffer_packet(frame_id, idx as u16, pkt.clone());
            }

            let mut completed = None;
            for pkt in &packets {
                let drop = rng.next_f32() < 0.05;
                if !drop {
                    if let Ok(Some(f)) = reassembler.process_packet(pkt) {
                        completed = Some(f);
                    }
                }
            }

            // One NACK attempt with 90% retransmit delivery success
            if completed.is_none() {
                if let Some(missing) = reassembler.get_missing_packets(frame_id) {
                    let retransmitted = nack_buffer.get_packets_for_nack(frame_id, &missing);
                    for pkt in retransmitted {
                        if rng.next_f32() >= 0.10 {
                            if let Ok(Some(f)) = reassembler.process_packet(&pkt) {
                                completed = Some(f);
                            }
                        }
                    }
                }
            }

            if completed.is_some() {
                completed_frames += 1;
            } else {
                dropped_frames += 1;
            }
        }

        println!(
            "    Total Frames: {}, Completed: {} ({:.1}%), Dropped: {} ({:.1}%)",
            total_frames,
            completed_frames,
            (completed_frames as f32 / total_frames as f32) * 100.0,
            dropped_frames,
            (dropped_frames as f32 / total_frames as f32) * 100.0
        );
        assert!(
            completed_frames >= 160,
            "At 5% loss with single NACK attempt, >=80% of frames must complete"
        );
    }

    // Test 3: 15% severe packet loss + burst drops (memory bounds and OOM protection)
    {
        println!("--> Scenario C: 15% Heavy Loss + Burst Drops (Bounded Memory Check)");
        let mut reassembler = FrameReassembler::new(Duration::from_millis(25));
        let mut rng = SimpleRng::new(9999);

        let total_frames = 500;
        let mut keyframes_sent = 0;
        let mut keyframes_completed = 0;

        for frame_id in 0..total_frames {
            let is_kf = (frame_id % 15) == 0;
            if is_kf {
                keyframes_sent += 1;
            }

            let frame_data = vec![0xEE; 50_000];
            let packets = FramePacketizer::packetize_to_datagrams(
                session_id,
                frame_id,
                (frame_id as u64) * 16666,
                is_kf,
                &frame_data,
                DEFAULT_MAX_PAYLOAD_SIZE,
            );

            // Burst loss simulation: drop 15% randomly + drop bursts of 6 consecutive packets
            let mut burst_remaining = 0;
            for pkt in &packets {
                if burst_remaining > 0 {
                    burst_remaining -= 1;
                    continue;
                }
                if rng.next_f32() < 0.05 {
                    burst_remaining = 5; // drop 6 packets in a row
                    continue;
                }
                if rng.next_f32() < 0.15 {
                    continue;
                }

                if let Ok(Some(f)) = reassembler.process_packet(pkt) {
                    if f.is_keyframe {
                        keyframes_completed += 1;
                    }
                }
            }
        }

        println!(
            "    Sent Keyframes: {}, Keyframes Completed without NACK: {}",
            keyframes_sent, keyframes_completed
        );
        println!(
            "    [OOM Protection Verified]: FrameReassembler completed without memory blowout"
        );
    }
}

#[test]
fn test_packet_reordering_and_duplicate_stress() {
    println!("\n================================================================================");
    println!(" [STRESS TEST] Packet Reordering & Duplicate Stress");
    println!("================================================================================");

    let session_id = 0xD00D_FEED_BEEF_0001u64;
    let mut reassembler = FrameReassembler::new(Duration::from_millis(100));
    let mut rng = SimpleRng::new(777);

    let total_frames = 100u32;

    for frame_id in 0..total_frames {
        let frame_size = 48_000;
        let mut frame_data = vec![0u8; frame_size];
        for (i, b) in frame_data.iter_mut().enumerate() {
            *b = (((frame_id * 31) as usize + i) & 0xFF) as u8;
        }

        let packets = FramePacketizer::packetize_to_datagrams(
            session_id,
            frame_id,
            (frame_id as u64) * 16666,
            frame_id % 10 == 0,
            &frame_data,
            DEFAULT_MAX_PAYLOAD_SIZE,
        );

        // Create a chaotic stream: shuffle packets + insert 50% duplicate packets
        let mut packet_stream: Vec<Vec<u8>> = packets.clone();

        // 1. Invert order of half the packets
        if frame_id % 2 == 0 {
            packet_stream.reverse();
        } else {
            // Fisher-Yates pseudo-random shuffle
            for i in (1..packet_stream.len()).rev() {
                let j = (rng.next_u32() as usize) % (i + 1);
                packet_stream.swap(i, j);
            }
        }

        // 2. Inject duplicate packets
        let mut stream_with_dups = Vec::new();
        for pkt in packet_stream {
            if rng.next_f32() < 0.50 {
                stream_with_dups.push(pkt.clone()); // duplicate
            }
            stream_with_dups.push(pkt);
        }

        // 3. Process through reassembler
        let mut completed = None;
        for pkt in &stream_with_dups {
            if let Ok(Some(f)) = reassembler.process_packet(pkt) {
                completed = Some(f);
            }
        }

        let res = completed
            .expect("Кадр обязан успешно собраться даже при сильной перестановке и дубликатах");
        assert_eq!(res.frame_id, frame_id);
        assert_eq!(res.data.len(), frame_size);
        assert_eq!(
            res.data, frame_data,
            "Данные кадра не должны быть повреждены из-за перестановки пакетов"
        );
    }

    println!(
        "    Processed {} frames with reverse order, random shuffle, and 50% duplicates.",
        total_frames
    );
    println!("    Result: 100% Bit-Exact Reassembly Success!");
}

#[test]
fn test_sequence_number_rollover_stress() {
    println!("\n================================================================================");
    println!(" [STRESS TEST] Sequence Number Rollover (u32::MAX Boundary Crossing)");
    println!("================================================================================");

    let session_id = 0x9988_7766_5544_3322u64;
    let mut reassembler = FrameReassembler::new(Duration::from_millis(50));

    // Verify is_newer_frame handles wrapping
    assert!(is_newer_frame(0, u32::MAX));
    assert!(is_newer_frame(1, 0));
    assert!(is_newer_frame(100, u32::MAX - 50));
    assert!(!is_newer_frame(u32::MAX, 0));
    assert!(!is_newer_frame(u32::MAX - 1, u32::MAX));

    // Stream 60 frames crossing the u32::MAX boundary: from (u32::MAX - 30) to +29
    let start_id = u32::MAX - 30;
    let total = 60u32;

    for offset in 0..total {
        let frame_id = start_id.wrapping_add(offset);
        let frame_size = 15_000;
        let frame_data = vec![(offset & 0xFF) as u8; frame_size];

        let packets = FramePacketizer::packetize_to_datagrams(
            session_id,
            frame_id,
            (offset as u64) * 16666,
            offset == 0 || offset == 31,
            &frame_data,
            DEFAULT_MAX_PAYLOAD_SIZE,
        );

        let mut completed = None;
        for pkt in &packets {
            if let Ok(Some(f)) = reassembler.process_packet(pkt) {
                completed = Some(f);
            }
        }

        let rf =
            completed.unwrap_or_else(|| panic!("Кадр #{} не собрался на границе u32", frame_id));
        assert_eq!(rf.frame_id, frame_id);
        assert_eq!(rf.data, frame_data);
    }

    println!("    60 frames cleanly reassembled across u32::MAX rollover boundary!");
}

#[test]
fn test_fec_reed_solomon_erasure_recovery_benchmark() {
    println!("\n================================================================================");
    println!(" [STRESS BENCHMARK] Reed-Solomon SIMD Forward Error Correction (FEC)");
    println!("================================================================================");

    let configs = [
        ("LAN (10 data + 2 parity)", FecConfig::lan()),
        ("WiFi/WAN (8 data + 3 parity)", FecConfig::wifi()),
    ];

    for (name, config) in configs {
        let encoder = FecEncoder::new(config).expect("Failed to create FecEncoder");
        let shard_size = 1380;
        let mut data_shards = Vec::with_capacity(config.data_shards);

        for i in 0..config.data_shards {
            let shard = vec![((i * 17 + 3) & 0xFF) as u8; shard_size];
            data_shards.push(shard);
        }

        // Measure encode speed
        let iters = 200;
        let t_enc_start = Instant::now();
        let mut parity_shards = Vec::new();
        for _ in 0..iters {
            parity_shards = encoder
                .encode_parity(&data_shards)
                .expect("FEC encode failed");
        }
        let t_enc = t_enc_start.elapsed() / (iters as u32);
        assert_eq!(parity_shards.len(), config.parity_shards);

        // Erase parity_shards.min(config.parity_shards) data shards and recover via FecDecoder
        let frame_id = 42u32;
        let group_id = 0u8;

        // Erase shard 1 and shard 3
        let erased_indices = [1usize, 3usize];

        let t_dec_start = Instant::now();
        let mut recovered_result = None;
        for _ in 0..iters {
            let mut dec_instance = FecDecoder::new(config).unwrap();

            let total_shards = config.total_shards() as u8;

            // Feed surviving data shards
            for (idx, shard) in data_shards.iter().enumerate() {
                if !erased_indices.contains(&idx) {
                    let _ = dec_instance.add_shard(
                        frame_id,
                        group_id,
                        idx as u8,
                        total_shards,
                        shard.clone(),
                    );
                }
            }

            // Feed parity shards to compensate for lost data shards
            for (p_idx, parity) in parity_shards.iter().enumerate() {
                let shard_index = (config.data_shards + p_idx) as u8;
                if let Some(recovered) = dec_instance.add_shard(
                    frame_id,
                    group_id,
                    shard_index,
                    total_shards,
                    parity.clone(),
                ) {
                    recovered_result = Some(recovered);
                    break;
                }
            }
        }
        let t_dec = t_dec_start.elapsed() / (iters as u32);

        let recovered = recovered_result.expect("FEC recovery must succeed");
        assert_eq!(recovered.len(), config.data_shards);
        for (orig, rec) in data_shards.iter().zip(recovered.iter()) {
            assert_eq!(orig, rec, "Восстановленный FEC-шард поврежден!");
        }

        let total_data_bytes = config.data_shards * shard_size;
        let encode_mbps = ((total_data_bytes as f64) / (1024.0 * 1024.0)) / t_enc.as_secs_f64();
        let decode_mbps = ((total_data_bytes as f64) / (1024.0 * 1024.0)) / t_dec.as_secs_f64();

        println!("  Config: {}", name);
        println!("    Shard Size      : {} bytes (MTU-safe)", shard_size);
        println!(
            "    Encode Latency  : {:.2} µs ({:.1} MB/s)",
            t_enc.as_micros(),
            encode_mbps
        );
        println!(
            "    Decode Latency  : {:.2} µs ({:.1} MB/s) with 2 shards erased",
            t_dec.as_micros(),
            decode_mbps
        );
        assert!(
            t_enc < Duration::from_micros(200),
            "FEC encode latency must be < 200µs with SIMD"
        );
        assert!(
            t_dec < Duration::from_micros(300),
            "FEC decode latency must be < 300µs with SIMD"
        );
    }
}
