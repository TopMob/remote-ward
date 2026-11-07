use core_protocol::{ButtonState, InputEvent, InputPacket, MouseButton};
use host_input::WindowsInputInjector;
use std::time::{Duration, Instant};

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
fn test_input_serialization_and_injection_stress() {
    println!("\n================================================================================");
    println!(" [STRESS TEST] High Frequency Input Serialization & Injection (20,000 Events)");
    println!("================================================================================");

    let injector = WindowsInputInjector::with_dimensions(2560, 1440);
    let session_id = 0x1234_5678_9ABC_DEF0u64;

    let event_count = 10_000usize;
    let mut mouse_latencies = Vec::with_capacity(event_count);
    let mut keyboard_latencies = Vec::with_capacity(event_count);

    let total_start = Instant::now();

    // 1. High frequency Mouse Movements (relative and absolute)
    for i in 0..event_count {
        let event = if i % 2 == 0 {
            InputEvent::MouseMoveRelative {
                dx: ((i as i32 % 21) - 10),
                dy: (((i * 3) as i32 % 21) - 10),
            }
        } else {
            InputEvent::MouseMoveAbsolute {
                x: ((i * 17) % 2560) as u32,
                y: ((i * 13) % 1440) as u32,
            }
        };

        let t0 = Instant::now();
        // Network packetization round-trip
        let pkt = InputPacket::new(session_id, i as u32, event.clone());
        let bytes = pkt.to_packet().expect("Serialization failed");
        let parsed = InputPacket::from_packet(&bytes).expect("Deserialization failed");
        assert_eq!(parsed.session_id, session_id);
        assert_eq!(parsed.sequence, i as u32);

        // Inject into OS via WindowsInputInjector
        let _ = injector.inject(&parsed.event);
        mouse_latencies.push(t0.elapsed());
    }

    // 2. High frequency Keyboard & Mouse Button toggles
    let keys = [
        (0x11, false), // W
        (0x1E, false), // A
        (0x1F, false), // S
        (0x20, false), // D
        (0x2A, false), // LShift
        (0x1D, false), // LCtrl
        (0x38, false), // LAlt
        (0x39, false), // Space
        (0x48, true),  // Up Arrow (extended)
        (0x50, true),  // Down Arrow (extended)
    ];

    for i in 0..event_count {
        let (scan_code, is_ext) = keys[i % keys.len()];
        let state = if (i / keys.len()) % 2 == 0 {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };

        let event = InputEvent::Keyboard {
            scan_code,
            is_extended: is_ext,
            state,
        };

        let t0 = Instant::now();
        let pkt = InputPacket::new(session_id, (event_count + i) as u32, event);
        let bytes = pkt.to_packet().expect("Serialization failed");
        let parsed = InputPacket::from_packet(&bytes).expect("Deserialization failed");

        let _ = injector.inject(&parsed.event);
        keyboard_latencies.push(t0.elapsed());
    }

    let total_elapsed = total_start.elapsed();
    let total_events = event_count * 2;
    let events_per_sec = (total_events as f64) / total_elapsed.as_secs_f64();

    let mouse_avg = mouse_latencies.iter().sum::<Duration>() / (event_count as u32);
    let mouse_p50 = percentile(&mut mouse_latencies, 50.0);
    let mouse_p95 = percentile(&mut mouse_latencies, 95.0);
    let mouse_p99 = percentile(&mut mouse_latencies, 99.0);

    let kbd_avg = keyboard_latencies.iter().sum::<Duration>() / (event_count as u32);
    let kbd_p50 = percentile(&mut keyboard_latencies, 50.0);
    let kbd_p95 = percentile(&mut keyboard_latencies, 95.0);
    let kbd_p99 = percentile(&mut keyboard_latencies, 99.0);

    println!("Total Input Events Injected : {}", total_events);
    println!(
        "Total Execution Time        : {:.2} ms",
        total_elapsed.as_secs_f64() * 1000.0
    );
    println!(
        "Input Injection Throughput  : {:.0} events/sec",
        events_per_sec
    );
    println!(
        "Mouse Event Latency         : avg={:.1}µs, p50={:.1}µs, p95={:.1}µs, p99={:.1}µs",
        mouse_avg.as_micros(),
        mouse_p50.as_micros(),
        mouse_p95.as_micros(),
        mouse_p99.as_micros()
    );
    println!(
        "Keyboard Event Latency      : avg={:.1}µs, p50={:.1}µs, p95={:.1}µs, p99={:.1}µs",
        kbd_avg.as_micros(),
        kbd_p50.as_micros(),
        kbd_p95.as_micros(),
        kbd_p99.as_micros()
    );

    // Stuck keys release verification
    injector.release_all();
    assert!(
        injector.pressed_keys().is_empty(),
        "All pressed keys must be cleared after release_all()"
    );
    assert!(
        injector.pressed_mouse_buttons().is_empty(),
        "All pressed buttons must be cleared after release_all()"
    );

    assert!(
        events_per_sec > 5_000.0,
        "Throughput must exceed 5,000 input events/sec"
    );
    assert!(
        mouse_p99 < Duration::from_micros(500),
        "p99 mouse event latency must be < 500µs"
    );
    assert!(
        kbd_p99 < Duration::from_micros(500),
        "p99 keyboard event latency must be < 500µs"
    );
}

#[test]
fn test_stuck_keys_tracking_and_release_all() {
    println!("\n================================================================================");
    println!(" [STRESS TEST] Stuck Keys Tracking & Clean release_all Verification");
    println!("================================================================================");

    let injector = WindowsInputInjector::with_dimensions(2560, 1440);

    // Initial state: nothing pressed
    assert!(injector.pressed_keys().is_empty());
    assert!(injector.pressed_mouse_buttons().is_empty());

    // 1. Simulate user holding W, A, S, D, Left Shift, Left Ctrl
    let pressed_keys_spec = [
        (0x11u16, false), // W
        (0x1Eu16, false), // A
        (0x1Fu16, false), // S
        (0x20u16, false), // D
        (0x2Au16, false), // LShift
        (0x1Du16, false), // LCtrl
        (0x5Bu16, true),  // LWin (extended)
    ];

    for (sc, is_ext) in &pressed_keys_spec {
        let _ = injector.inject(&InputEvent::Keyboard {
            scan_code: *sc,
            is_extended: *is_ext,
            state: ButtonState::Pressed,
        });
    }

    // 2. Simulate holding Left, Right, and Middle Mouse Buttons
    let mouse_buttons_spec = [MouseButton::Left, MouseButton::Right, MouseButton::Middle];
    for btn in &mouse_buttons_spec {
        let _ = injector.inject(&InputEvent::MouseButton {
            button: *btn,
            state: ButtonState::Pressed,
        });
    }

    // Verify all keys and buttons are tracked in the internal state
    let held_keys = injector.pressed_keys();
    let held_buttons = injector.pressed_mouse_buttons();

    println!(
        "    Currently tracked pressed keys   : {} keys",
        held_keys.len()
    );
    println!(
        "    Currently tracked mouse buttons  : {} buttons",
        held_buttons.len()
    );

    assert_eq!(held_keys.len(), pressed_keys_spec.len());
    for (sc, is_ext) in &pressed_keys_spec {
        assert!(
            injector.is_key_pressed(*sc, *is_ext),
            "Key (sc={:#x}, ext={}) must be marked pressed",
            sc,
            is_ext
        );
    }

    assert_eq!(held_buttons.len(), mouse_buttons_spec.len());
    for btn in &mouse_buttons_spec {
        assert!(
            injector.is_mouse_button_pressed(*btn),
            "Button {:?} must be marked pressed",
            btn
        );
    }

    // 3. User releases 'A' and 'Left Mouse Button' cleanly before disconnect
    let _ = injector.inject(&InputEvent::Keyboard {
        scan_code: 0x1E,
        is_extended: false,
        state: ButtonState::Released,
    });
    let _ = injector.inject(&InputEvent::MouseButton {
        button: MouseButton::Left,
        state: ButtonState::Released,
    });

    assert!(!injector.is_key_pressed(0x1E, false));
    assert!(!injector.is_mouse_button_pressed(MouseButton::Left));
    assert_eq!(injector.pressed_keys().len(), pressed_keys_spec.len() - 1);
    assert_eq!(
        injector.pressed_mouse_buttons().len(),
        mouse_buttons_spec.len() - 1
    );

    // 4. Client disconnects abruptly! Call release_all()
    let t0 = Instant::now();
    injector.release_all();
    let release_latency = t0.elapsed();

    println!(
        "    release_all execution latency    : {:.2} µs",
        release_latency.as_micros()
    );

    // 5. Verify ALL keys and buttons are 100% cleared
    assert!(
        injector.pressed_keys().is_empty(),
        "pressed_keys must be empty after release_all"
    );
    assert!(
        injector.pressed_mouse_buttons().is_empty(),
        "pressed_mouse_buttons must be empty after release_all"
    );

    for (sc, is_ext) in &pressed_keys_spec {
        assert!(
            !injector.is_key_pressed(*sc, *is_ext),
            "Key (sc={:#x}, ext={}) must be released",
            sc,
            is_ext
        );
    }
    for btn in &mouse_buttons_spec {
        assert!(
            !injector.is_mouse_button_pressed(*btn),
            "Button {:?} must be released",
            btn
        );
    }

    println!("    Stuck Keys Release Verification  : PASSED (Zero remaining stuck keys)");
}
