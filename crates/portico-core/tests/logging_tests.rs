use portico_core::logging::{DaemonEvent, LogLevel, LogRingBuffer};

#[test]
fn ring_buffer_caps_capacity_and_filters_since_seq() {
    let ring = LogRingBuffer::new(3);
    let e1 = ring.push(LogLevel::Info, "core", "one");
    let _e2 = ring.push(LogLevel::Warn, "core", "two");
    let e3 = ring.push(LogLevel::Info, "core", "three");
    let e4 = ring.push(LogLevel::Error, "core", "four");
    let e5 = ring.push(LogLevel::Info, "core", "five");

    assert_eq!(e1.seq, 1);
    assert_eq!(e5.seq, 5);

    let all = ring.since(0);
    assert_eq!(all.len(), 3);
    assert_eq!(all.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![3, 4, 5]);

    let after3 = ring.since(e3.seq);
    assert_eq!(
        after3.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![e4.seq, e5.seq]
    );
}

#[test]
fn event_control_line_escapes_newlines_and_spaces() {
    let ev = DaemonEvent::DeviceAdded {
        bus_id: "1-2".into(),
        vid: 0x0781,
        pid: 0x5583,
        product: "SanDisk Ultra\nUSB 3.0".into(),
        state: "shared".into(),
    };
    let line = ev.to_control_line();
    assert!(!line.contains('\n'));
    assert!(line.starts_with("EVENT DEVICE_ADDED "));
    assert!(line.contains("vid=0781"));
    assert!(line.contains("pid=5583"));
}
