use portico_core::daemon::PorticoDaemon;
use portico_core::usb_handler::transfer_timeout;
use portico_proto::frame::{
    read_frame, write_frame, AuthKind, Frame, HelloStatus, TunnelStatus, CAP_USBIP_V1,
};
use portico_proto::tls::{build_client_tls_config, configure_tunnel_tcp, CertCheckPolicy};
use std::any::Any;
use std::io::{Error as IoError, ErrorKind, Result as IoResult};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use usbip::usbip_protocol::{UsbIpCommand, UsbIpHeaderBasic, USBIP_CMD_SUBMIT};
use usbip::{
    Direction, EndpointAttributes, SetupPacket, UsbDevice, UsbDeviceHandler, UsbEndpoint,
    UsbInterface, UsbInterfaceHandler, UsbSpeed,
};

#[derive(Clone, Debug)]
struct MockMassStorageDeviceHandler {
    clear_halt_count: Arc<AtomicUsize>,
}

impl UsbDeviceHandler for MockMassStorageDeviceHandler {
    fn clone_box(&self) -> Option<Box<dyn UsbDeviceHandler + Send>> {
        Some(Box::new(self.clone()))
    }

    fn handle_urb(
        &mut self,
        _transfer_buffer_length: u32,
        setup: SetupPacket,
        _req: &[u8],
    ) -> IoResult<Vec<u8>> {
        if setup.request_type == 0x02 && setup.request == 0x01 && setup.value == 0 {
            self.clear_halt_count.fetch_add(1, Ordering::AcqRel);
            return Ok(Vec::new());
        }
        Ok(Vec::new())
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Clone, Debug)]
struct MockMassStorageHandler {
    pending_cmd: Arc<AtomicU8>,
    pending_tag: Arc<Mutex<u32>>,
    pending_len: Arc<AtomicUsize>,
    pending_lba: Arc<AtomicUsize>,
    stalled: Arc<AtomicBool>,
    release_count: Arc<AtomicUsize>,
    block_bulk_in: Option<Duration>,
    ep_in_delay: Option<Duration>,
    cancel_wait: Arc<(Mutex<bool>, Condvar)>,
}

impl MockMassStorageHandler {
    fn new() -> Self {
        Self {
            pending_cmd: Arc::new(AtomicU8::new(0)),
            pending_tag: Arc::new(Mutex::new(0)),
            pending_len: Arc::new(AtomicUsize::new(0)),
            pending_lba: Arc::new(AtomicUsize::new(0)),
            stalled: Arc::new(AtomicBool::new(false)),
            release_count: Arc::new(AtomicUsize::new(0)),
            block_bulk_in: None,
            ep_in_delay: None,
            cancel_wait: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }
}

impl UsbInterfaceHandler for MockMassStorageHandler {
    fn get_class_specific_descriptor(&self) -> Vec<u8> {
        Vec::new()
    }

    fn clone_box(&self) -> Option<Box<dyn UsbInterfaceHandler + Send>> {
        Some(Box::new(self.clone()))
    }

    fn on_tunnel_release(&self) {
        self.release_count.fetch_add(1, Ordering::AcqRel);
        let (lock, cv) = &*self.cancel_wait;
        *lock.lock().unwrap() = true;
        cv.notify_all();
    }

    fn handle_urb(
        &mut self,
        _interface: &UsbInterface,
        ep: UsbEndpoint,
        transfer_buffer_length: u32,
        setup: SetupPacket,
        req: &[u8],
    ) -> IoResult<Vec<u8>> {
        if ep.attributes == EndpointAttributes::Control as u8 {
            // GET_MAX_LUN
            if setup.request_type == 0xA1 && setup.request == 0xFE {
                return Ok(vec![0]);
            }
            return Ok(Vec::new());
        }

        if ep.attributes == EndpointAttributes::Bulk as u8 {
            if let Direction::Out = ep.direction() {
                // BOT CBW (31 bytes, signature USBC = 0x43425355 LE)
                if req.len() == 31 && &req[0..4] == b"USBC" {
                    let tag = u32::from_le_bytes(req[4..8].try_into().unwrap());
                    let data_len = u32::from_le_bytes(req[8..12].try_into().unwrap()) as usize;
                    let opcode = req[15];
                    *self.pending_tag.lock().unwrap() = tag;
                    self.pending_cmd.store(opcode, Ordering::Release);
                    self.pending_len.store(data_len, Ordering::Release);
                    self.stalled.store(false, Ordering::Release);
                    if opcode == 0x28 && req.len() >= 25 {
                        // READ(10) LBA
                        let lba = u32::from_be_bytes(req[17..21].try_into().unwrap()) as usize;
                        self.pending_lba.store(lba, Ordering::Release);
                    } else if opcode == 0x1A && req.get(17) == Some(&0x08) {
                        // MODE SENSE(6) caching page 0x08 -> pendrive stalls Bulk IN
                        self.stalled.store(true, Ordering::Release);
                    }
                }
                return Ok(Vec::new());
            } else {
                // Bulk IN
                if let Some(d) = self.block_bulk_in {
                    let (lock, cv) = &*self.cancel_wait;
                    let guard = lock.lock().unwrap();
                    let (g, _) = cv.wait_timeout_while(guard, d, |cancelled| !*cancelled).unwrap();
                    if *g {
                        return Err(IoError::new(ErrorKind::ConnectionReset, "usb urb aborted on release"));
                    }
                }
                if let Some(d) = self.ep_in_delay {
                    std::thread::sleep(d);
                }
                let len = transfer_buffer_length as usize;
                // CSW (13 bytes)
                if len == 13 {
                    let tag = *self.pending_tag.lock().unwrap();
                    let status = if self.stalled.swap(false, Ordering::AcqRel) {
                        1u8 // CHECK CONDITION
                    } else {
                        0u8
                    };
                    let mut csw = Vec::with_capacity(13);
                    csw.extend_from_slice(b"USBS");
                    csw.extend_from_slice(&tag.to_le_bytes());
                    csw.extend_from_slice(&0u32.to_le_bytes());
                    csw.push(status);
                    return Ok(csw);
                }

                if self.stalled.load(Ordering::Acquire) {
                    return Err(IoError::new(ErrorKind::BrokenPipe, "usb bulk stall"));
                }

                let opcode = self.pending_cmd.load(Ordering::Acquire);
                let data_len = self.pending_len.load(Ordering::Acquire).min(len);
                let mut out = vec![0u8; data_len];
                match opcode {
                    0x12 => {
                        // INQUIRY
                        if out.len() >= 36 {
                            out[0] = 0x00;
                            out[8..16].copy_from_slice(b"SanDisk ");
                        }
                    }
                    0x25 => {
                        // READ CAPACITY(10): 240328704 blocks, 512 bytes/block
                        if out.len() >= 8 {
                            out[0..4].copy_from_slice(&240_328_703u32.to_be_bytes());
                            out[4..8].copy_from_slice(&512u32.to_be_bytes());
                        }
                    }
                    0x28 => {
                        // READ(10): deterministic pattern keyed by LBA
                        let lba = self.pending_lba.load(Ordering::Acquire) as u8;
                        for (i, b) in out.iter_mut().enumerate() {
                            *b = lba.wrapping_add((i & 0xFF) as u8);
                        }
                    }
                    _ => {}
                }
                return Ok(out);
            }
        }
        Ok(Vec::new())
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

fn make_mock_pendrive(
    bus_id: &str,
    handler: MockMassStorageHandler,
    clear_halt_count: Arc<AtomicUsize>,
) -> UsbDevice {
    let mut d = UsbDevice::new(1);
    d.bus_id = bus_id.to_string();
    d.vendor_id = 0x0781;
    d.product_id = 0x5595;
    d.speed = UsbSpeed::Super as u32;
    d.usb_version = rusb::Version(3, 2, 0).into();
    d.set_ep0_max_packet_size(9);
    d = d
        .with_interface(
            0x08, // Mass Storage
            0x06, // SCSI
            0x50, // BOT
            Some("Bulk-Only"),
            vec![
                UsbEndpoint {
                    address: 0x81,
                    attributes: EndpointAttributes::Bulk as u8,
                    max_packet_size: 1024,
                    interval: 0,
                },
                UsbEndpoint {
                    address: 0x02,
                    attributes: EndpointAttributes::Bulk as u8,
                    max_packet_size: 1024,
                    interval: 0,
                },
            ],
            Arc::new(Mutex::new(Box::new(handler))),
        )
        .with_device_handler(Arc::new(Mutex::new(Box::new(
            MockMassStorageDeviceHandler { clear_halt_count },
        ))));
    d.interfaces[0].endpoint_extras = vec![
        vec![0x06, 0x30, 0x0F, 0x00, 0x00, 0x00],
        vec![0x06, 0x30, 0x0F, 0x00, 0x00, 0x00],
    ];
    d
}

async fn connect_and_open_tunnel(
    addr: SocketAddr,
    fp: &str,
    token: &str,
    bus_id: &str,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, TunnelStatus> {
    let (cfg, _) = build_client_tls_config(
        None,
        CertCheckPolicy::PinnedOrFirstUse {
            expected_fp: Some(fp.to_string()),
            trust_first_use: false,
        },
    )
    .unwrap();
    let tcp = TcpStream::connect(addr).await.unwrap();
    configure_tunnel_tcp(&tcp);
    let mut tls = TlsConnector::from(cfg)
        .connect("localhost".try_into().unwrap(), tcp)
        .await
        .unwrap();
    write_frame(
        &mut tls,
        &Frame::ClientHello {
            min_ver: 0,
            max_ver: 0,
            auth_kind: AuthKind::ClaimToken,
            credentials: token.as_bytes().to_vec(),
            client_name: "test-cli".into(),
            capabilities: CAP_USBIP_V1,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        read_frame(&mut tls).await.unwrap(),
        Frame::ServerHello {
            status: HelloStatus::Ok,
            ..
        }
    ));
    write_frame(
        &mut tls,
        &Frame::OpenTunnelReq {
            bus_id: bus_id.to_string(),
        },
    )
    .await
    .unwrap();
    match read_frame(&mut tls).await.unwrap() {
        Frame::OpenTunnelResp {
            status: TunnelStatus::Ok,
            ..
        } => Ok(tls),
        Frame::OpenTunnelResp { status, .. } => Err(status),
        other => panic!("unexpected OpenTunnelResp: {other:?}"),
    }
}

async fn usbip_import(tls: &mut tokio_rustls::client::TlsStream<TcpStream>, bus_id: &str) {
    let mut bus = bus_id.as_bytes().to_vec();
    bus.resize(32, 0);
    let import = UsbIpCommand::OpReqImport {
        status: 0,
        busid: bus.try_into().unwrap(),
    }
    .to_bytes();
    tls.write_all(&import).await.unwrap();
    let _ver = tls.read_u32().await.unwrap();
    assert_eq!(tls.read_u32().await.unwrap(), 0);
    let mut info = vec![0u8; 0x138];
    tls.read_exact(&mut info).await.unwrap();
}

async fn submit_urb(
    tls: &mut tokio_rustls::client::TlsStream<TcpStream>,
    seqnum: u32,
    direction: u32,
    ep: u32,
    transfer_len: u32,
    setup: [u8; 8],
    data: Vec<u8>,
) -> (i32, Vec<u8>) {
    let cmd = UsbIpCommand::UsbIpCmdSubmit {
        header: UsbIpHeaderBasic {
            command: USBIP_CMD_SUBMIT.into(),
            seqnum,
            devid: 0,
            direction,
            ep,
        },
        transfer_flags: 0,
        transfer_buffer_length: transfer_len,
        start_frame: 0,
        number_of_packets: 0,
        interval: 0,
        setup,
        data,
        iso_packet_descriptor: vec![],
    }
    .to_bytes();
    tls.write_all(&cmd).await.unwrap();

    let mut hdr = [0u8; 48];
    tls.read_exact(&mut hdr).await.unwrap();
    let resp_seq = u32::from_be_bytes(hdr[4..8].try_into().unwrap());
    assert_eq!(resp_seq, seqnum);
    let status = i32::from_be_bytes(hdr[20..24].try_into().unwrap());
    let actual_len = u32::from_be_bytes(hdr[24..28].try_into().unwrap()) as usize;
    let payload = if direction == 1 && actual_len > 0 {
        let mut buf = vec![0u8; actual_len];
        tls.read_exact(&mut buf).await.unwrap();
        buf
    } else {
        Vec::new()
    };
    (status, payload)
}

fn make_bot_cbw(tag: u32, data_len: u32, flags: u8, cdb: &[u8]) -> Vec<u8> {
    let mut cbw = vec![0u8; 31];
    cbw[0..4].copy_from_slice(b"USBC");
    cbw[4..8].copy_from_slice(&tag.to_le_bytes());
    cbw[8..12].copy_from_slice(&data_len.to_le_bytes());
    cbw[12] = flags;
    cbw[14] = cdb.len() as u8;
    cbw[15..15 + cdb.len()].copy_from_slice(cdb);
    cbw
}

#[test]
fn usb_handler_timeouts_provide_ample_margin() {
    assert!(transfer_timeout(512) >= Duration::from_secs(10));
    assert!(transfer_timeout(1024 * 1024) >= Duration::from_secs(14));
    assert_eq!(transfer_timeout(16 * 1024 * 1024), Duration::from_secs(60));
}

#[tokio::test]
async fn bulk_mass_storage_probe_stall_epipe_and_24_sequential_1mb_reads_benchmark() {
    let dir = TempDir::new().unwrap();
    let daemon = PorticoDaemon::start_with_bind_overrides(
        dir.path().to_path_buf(),
        dir.path().join("portico-ctrl.sock"),
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
    )
    .await
    .unwrap();
    let h = daemon.handle();

    let clear_halt_count = Arc::new(AtomicUsize::new(0));
    let mock = MockMassStorageHandler::new();
    let dev = make_mock_pendrive("1-1", mock, clear_halt_count.clone());
    h.state
        .registry
        .register_usb_device(dev, "SanDisk".into(), "USB 3.2".into(), "S1".into(), &[])
        .await
        .unwrap();

    assert_eq!(h.exec_command("START_SERVER").await, "OK");
    let addr = h.network.bound_tcp_addr().await.unwrap();
    let token = h
        .exec_command("GET_TOKEN")
        .await
        .strip_prefix("OK ")
        .unwrap()
        .to_string();
    let fp = h.state.identity.read().await.fingerprint_hex();

    let t_probe_start = Instant::now();
    let mut tls = connect_and_open_tunnel(addr, &fp, &token, "1-1")
        .await
        .unwrap();
    usbip_import(&mut tls, "1-1").await;

    let mut seq = 1u32;
    // 1. Device descriptor: verify bcdUSB == 0x0320 and bMaxPacketSize0 == 9
    let (st, dev_desc) = submit_urb(
        &mut tls,
        seq,
        1,
        0,
        18,
        [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x12, 0x00],
        vec![],
    )
    .await;
    seq += 1;
    assert_eq!(st, 0);
    assert_eq!(dev_desc[2], 0x20);
    assert_eq!(dev_desc[3], 0x03);
    assert_eq!(dev_desc[7], 9);

    // 2. Config descriptor: verify SuperSpeed endpoint companion descriptor (0x30) is included
    let (st, cfg_desc) = submit_urb(
        &mut tls,
        seq,
        1,
        0,
        255,
        [0x80, 0x06, 0x00, 0x02, 0x00, 0x00, 0xFF, 0x00],
        vec![],
    )
    .await;
    seq += 1;
    assert_eq!(st, 0);
    assert_eq!(cfg_desc.len(), 9 + 9 + (7 + 6) * 2);
    assert_eq!(cfg_desc[26], 0x30);

    // 3. SET_CONFIGURATION (OUT, 0 bytes)
    let (st, _) = submit_urb(
        &mut tls,
        seq,
        0,
        0,
        0,
        [0x00, 0x09, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00],
        vec![],
    )
    .await;
    seq += 1;
    assert_eq!(st, 0);

    // 4. GET_MAX_LUN
    let (st, lun) = submit_urb(
        &mut tls,
        seq,
        1,
        0,
        1,
        [0xA1, 0xFE, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00],
        vec![],
    )
    .await;
    seq += 1;
    assert_eq!(st, 0);
    assert_eq!(lun, vec![0]);

    // 5. READ CAPACITY(10) -> 240328704 blocks
    let cbw = make_bot_cbw(1, 8, 0x80, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(submit_urb(&mut tls, seq, 0, 2, 31, [0; 8], cbw).await.0, 0);
    seq += 1;
    let (st, cap) = submit_urb(&mut tls, seq, 1, 1, 8, [0; 8], vec![]).await;
    seq += 1;
    assert_eq!(st, 0);
    assert_eq!(u32::from_be_bytes(cap[0..4].try_into().unwrap()), 240_328_703);
    let (st, csw) = submit_urb(&mut tls, seq, 1, 1, 13, [0; 8], vec![]).await;
    seq += 1;
    assert_eq!(st, 0);
    assert_eq!(csw[12], 0);

    // 6. MODE SENSE(6) caching page 0x08 -> STALL MUST return -EPIPE (-32), NOT 1
    let cbw = make_bot_cbw(2, 192, 0x80, &[0x1A, 0x00, 0x08, 0x00, 192, 0x00]);
    assert_eq!(submit_urb(&mut tls, seq, 0, 2, 31, [0; 8], cbw).await.0, 0);
    seq += 1;
    let (st, _) = submit_urb(&mut tls, seq, 1, 1, 192, [0; 8], vec![]).await;
    seq += 1;
    assert_eq!(st, -32, "USB endpoint STALL must map to -EPIPE (-32)");
    // Kernel clears halt on 0x81, then reads CSW (CHECK CONDITION = 1)
    let (st, _) = submit_urb(
        &mut tls,
        seq,
        0,
        0,
        0,
        [0x02, 0x01, 0x00, 0x00, 0x81, 0x00, 0x00, 0x00],
        vec![],
    )
    .await;
    seq += 1;
    assert_eq!(st, 0);
    assert_eq!(clear_halt_count.load(Ordering::Acquire), 1);
    let (st, csw) = submit_urb(&mut tls, seq, 1, 1, 13, [0; 8], vec![]).await;
    seq += 1;
    assert_eq!(st, 0);
    assert_eq!(csw[12], 1);

    // 7. Partition table read (4KB at LBA 0)
    let cbw = make_bot_cbw(3, 4096, 0x80, &[0x28, 0, 0, 0, 0, 0, 0, 0, 8, 0]);
    assert_eq!(submit_urb(&mut tls, seq, 0, 2, 31, [0; 8], cbw).await.0, 0);
    seq += 1;
    let (st, mbr) = submit_urb(&mut tls, seq, 1, 1, 4096, [0; 8], vec![]).await;
    seq += 1;
    assert_eq!(st, 0);
    assert_eq!(mbr.len(), 4096);
    let (st, csw) = submit_urb(&mut tls, seq, 1, 1, 13, [0; 8], vec![]).await;
    seq += 1;
    assert_eq!(st, 0);
    assert_eq!(csw[12], 0);

    let probe_elapsed = t_probe_start.elapsed();
    eprintln!("[bench] probe + partition table read: {probe_elapsed:?}");
    assert!(
        probe_elapsed < Duration::from_secs(5),
        "probe took too long: {probe_elapsed:?}"
    );

    // 8. Benchmark 24 sequential 1MB bulk reads (24 MiB)
    let t_read_start = Instant::now();
    for iter in 0..24u32 {
        let lba = iter * 2048;
        let cbw = make_bot_cbw(
            10 + iter,
            1024 * 1024,
            0x80,
            &[
                0x28,
                0,
                (lba >> 24) as u8,
                (lba >> 16) as u8,
                (lba >> 8) as u8,
                lba as u8,
                0,
                0x08,
                0x00,
                0,
            ],
        );
        assert_eq!(submit_urb(&mut tls, seq, 0, 2, 31, [0; 8], cbw).await.0, 0);
        seq += 1;
        let (st, blk) = submit_urb(&mut tls, seq, 1, 1, 1024 * 1024, [0; 8], vec![]).await;
        seq += 1;
        assert_eq!(st, 0);
        assert_eq!(blk.len(), 1024 * 1024);
        assert_eq!(blk[0], (lba & 0xFF) as u8);
        assert_eq!(blk[1024 * 1024 - 1], ((lba & 0xFF) as u8).wrapping_add(255));
        let (st, csw) = submit_urb(&mut tls, seq, 1, 1, 13, [0; 8], vec![]).await;
        seq += 1;
        assert_eq!(st, 0);
        assert_eq!(csw[12], 0);
    }
    let read_elapsed = t_read_start.elapsed();
    eprintln!(
        "[bench] 24x 1MB sequential reads (24 MiB) over TLS: {read_elapsed:?} ({:.1} MiB/s)",
        24.0 / read_elapsed.as_secs_f64()
    );
    assert!(
        read_elapsed < Duration::from_secs(10),
        "24x 1MB reads stalled: {read_elapsed:?}"
    );

    drop(tls);
    daemon.shutdown().await;
}

#[tokio::test]
async fn endpoint_pipelining_executes_concurrent_endpoints_without_blocking() {
    let dir = TempDir::new().unwrap();
    let daemon = PorticoDaemon::start_with_bind_overrides(
        dir.path().to_path_buf(),
        dir.path().join("portico-ctrl.sock"),
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
    )
    .await
    .unwrap();
    let h = daemon.handle();

    let mut mock = MockMassStorageHandler::new();
    mock.ep_in_delay = Some(Duration::from_millis(250));
    let dev = make_mock_pendrive("1-1", mock, Arc::new(AtomicUsize::new(0)));
    h.state
        .registry
        .register_usb_device(dev, "M".into(), "P".into(), "S".into(), &[])
        .await
        .unwrap();
    assert_eq!(h.exec_command("START_SERVER").await, "OK");

    let addr = h.network.bound_tcp_addr().await.unwrap();
    let token = h
        .exec_command("GET_TOKEN")
        .await
        .strip_prefix("OK ")
        .unwrap()
        .to_string();
    let fp = h.state.identity.read().await.fingerprint_hex();

    let mut tls = connect_and_open_tunnel(addr, &fp, &token, "1-1")
        .await
        .unwrap();
    usbip_import(&mut tls, "1-1").await;

    // Pipeline Bulk IN (seq=1, sleeps 250ms) and Control IN (seq=2, instant) back-to-back!
    let bulk_in_cmd = UsbIpCommand::UsbIpCmdSubmit {
        header: UsbIpHeaderBasic {
            command: USBIP_CMD_SUBMIT.into(),
            seqnum: 1,
            devid: 0,
            direction: 1,
            ep: 1,
        },
        transfer_flags: 0,
        transfer_buffer_length: 13,
        start_frame: 0,
        number_of_packets: 0,
        interval: 0,
        setup: [0; 8],
        data: vec![],
        iso_packet_descriptor: vec![],
    }
    .to_bytes();
    let ctrl_in_cmd = UsbIpCommand::UsbIpCmdSubmit {
        header: UsbIpHeaderBasic {
            command: USBIP_CMD_SUBMIT.into(),
            seqnum: 2,
            devid: 0,
            direction: 1,
            ep: 0,
        },
        transfer_flags: 0,
        transfer_buffer_length: 1,
        start_frame: 0,
        number_of_packets: 0,
        interval: 0,
        setup: [0xA1, 0xFE, 0, 0, 0, 0, 1, 0],
        data: vec![],
        iso_packet_descriptor: vec![],
    }
    .to_bytes();

    let t0 = Instant::now();
    tls.write_all(&bulk_in_cmd).await.unwrap();
    tls.write_all(&ctrl_in_cmd).await.unwrap();

    // First response MUST be seqnum 2 (Control IN) well before Bulk IN's 250ms delay!
    let mut hdr = [0u8; 48];
    tls.read_exact(&mut hdr).await.unwrap();
    let first_seq = u32::from_be_bytes(hdr[4..8].try_into().unwrap());
    let first_len = u32::from_be_bytes(hdr[24..28].try_into().unwrap()) as usize;
    let mut buf = vec![0u8; first_len];
    tls.read_exact(&mut buf).await.unwrap();
    assert_eq!(first_seq, 2, "Control URB must overtake delayed Bulk IN");
    assert!(
        t0.elapsed() < Duration::from_millis(150),
        "Control URB was blocked by Bulk IN: {:?}",
        t0.elapsed()
    );

    // Then Bulk IN (seqnum 1) arrives
    tls.read_exact(&mut hdr).await.unwrap();
    let second_seq = u32::from_be_bytes(hdr[4..8].try_into().unwrap());
    let second_len = u32::from_be_bytes(hdr[24..28].try_into().unwrap()) as usize;
    let mut buf2 = vec![0u8; second_len];
    tls.read_exact(&mut buf2).await.unwrap();
    assert_eq!(second_seq, 1);

    drop(tls);
    daemon.shutdown().await;
}

#[tokio::test]
async fn stale_claim_released_on_client_kill_even_with_blocked_urb_and_reattach_succeeds() {
    let dir = TempDir::new().unwrap();
    let daemon = PorticoDaemon::start_with_bind_overrides(
        dir.path().to_path_buf(),
        dir.path().join("portico-ctrl.sock"),
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
    )
    .await
    .unwrap();
    let h = daemon.handle();

    let mut mock = MockMassStorageHandler::new();
    mock.block_bulk_in = Some(Duration::from_secs(5));
    let release_count = mock.release_count.clone();
    let dev = make_mock_pendrive("1-1", mock, Arc::new(AtomicUsize::new(0)));
    h.state
        .registry
        .register_usb_device(dev, "M".into(), "P".into(), "S".into(), &[])
        .await
        .unwrap();
    assert_eq!(h.exec_command("START_SERVER").await, "OK");

    let addr = h.network.bound_tcp_addr().await.unwrap();
    let token = h
        .exec_command("GET_TOKEN")
        .await
        .strip_prefix("OK ")
        .unwrap()
        .to_string();
    let fp = h.state.identity.read().await.fingerprint_hex();

    // Client 1 claims device and submits a 5s-blocking Bulk IN URB
    let mut tls1 = connect_and_open_tunnel(addr, &fp, &token, "1-1")
        .await
        .unwrap();
    usbip_import(&mut tls1, "1-1").await;

    let blocking_cmd = UsbIpCommand::UsbIpCmdSubmit {
        header: UsbIpHeaderBasic {
            command: USBIP_CMD_SUBMIT.into(),
            seqnum: 1,
            devid: 0,
            direction: 1,
            ep: 1,
        },
        transfer_flags: 0,
        transfer_buffer_length: 13,
        start_frame: 0,
        number_of_packets: 0,
        interval: 0,
        setup: [0; 8],
        data: vec![],
        iso_packet_descriptor: vec![],
    }
    .to_bytes();
    tls1.write_all(&blocking_cmd).await.unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;

    // While Client 1 is alive, Client 2 is rejected with InUse
    let err2 = connect_and_open_tunnel(addr, &fp, &token, "1-1")
        .await
        .unwrap_err();
    assert_eq!(err2, TunnelStatus::InUse);

    // Kill Client 1 while the 5s URB is still blocked!
    let kill_time = Instant::now();
    drop(tls1);

    // Device must return to Shared within <1s (NOT 5s!) and invoke on_tunnel_release()
    let mut released = false;
    for _ in 0..40 {
        let list = h.exec_command("LIST_DEVICES").await;
        if list.contains("state=shared") {
            released = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        released,
        "stale claim was not released after client death; elapsed={:?}",
        kill_time.elapsed()
    );
    assert!(
        kill_time.elapsed() < Duration::from_secs(1),
        "claim release took too long: {:?}",
        kill_time.elapsed()
    );
    assert!(
        release_count.load(Ordering::Acquire) >= 1,
        "on_tunnel_release hook must run on tunnel death"
    );

    // New attach succeeds immediately
    let mut tls3 = connect_and_open_tunnel(addr, &fp, &token, "1-1")
        .await
        .unwrap();
    usbip_import(&mut tls3, "1-1").await;
    drop(tls3);

    daemon.shutdown().await;
}
