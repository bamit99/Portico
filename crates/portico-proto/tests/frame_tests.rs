use portico_proto::frame::{
    read_frame, write_frame, AuthKind, DeviceSummary, Frame, HelloStatus, LicenseTier,
    TunnelStatus, CAP_USBIP_V1,
};
use portico_proto::{ProtoError, MAX_FRAME_LEN};
use std::io::Cursor;

#[test]
fn hand_derived_client_hello_fixture_decodes_and_reencodes() {
    // Header: PRTC | ver=0 | type=0x01 | len=21
    // Payload: min=0 max=0 auth_kind=1 creds_len=3 "TOK" client_len=6 "laptop" caps=1
    let fixture: Vec<u8> = vec![
        b'P', b'R', b'T', b'C', 0x00, 0x01, 0x00, 0x00, 0x00, 20, // header (10B)
        0x00, 0x00, 0x01, // min, max, auth_kind
        0x00, 0x03, b'T', b'O', b'K', // credentials
        0x00, 0x06, b'l', b'a', b'p', b't', b'o', b'p', // client_name
        0x00, 0x00, 0x00, 0x01, // capabilities
    ];
    let decoded = Frame::decode(&fixture).expect("decode fixture");
    assert_eq!(
        decoded,
        Frame::ClientHello {
            min_ver: 0,
            max_ver: 0,
            auth_kind: AuthKind::ClaimToken,
            credentials: b"TOK".to_vec(),
            client_name: "laptop".to_string(),
            capabilities: CAP_USBIP_V1,
        }
    );
    assert_eq!(decoded.encode().unwrap(), fixture);
}

#[test]
fn all_frame_variants_roundtrip() {
    let frames = vec![
        Frame::ServerHello {
            chosen_ver: 0,
            status: HelloStatus::Ok,
            server_name: "phone".into(),
            capabilities: CAP_USBIP_V1,
            tier: LicenseTier::Free,
            message: String::new(),
        },
        Frame::ListDevicesReq,
        Frame::ListDevicesResp {
            devices: vec![DeviceSummary {
                bus_id: "1-2".into(),
                vid: 0x0781,
                pid: 0x5583,
                device_class: 0x08,
                device_subclass: 0x06,
                device_protocol: 0x50,
                speed: 3,
                manufacturer: "SanDisk".into(),
                product: "Ultra".into(),
                serial: "ABC123".into(),
                in_use: false,
            }],
        },
        Frame::OpenTunnelReq {
            bus_id: "1-2".into(),
        },
        Frame::OpenTunnelResp {
            status: TunnelStatus::Ignored,
            message: "ignored by policy".into(),
        },
        Frame::RequestReverseDataConn {
            conn_nonce: [7u8; 16],
            bus_id: "1-2".into(),
        },
        Frame::ReverseDataHello {
            conn_nonce: [9u8; 16],
        },
        Frame::Ping { nonce: 42 },
        Frame::Pong { nonce: 42 },
        Frame::Error {
            code: 9,
            message: "bad".into(),
        },
    ];

    for f in frames {
        let bytes = f.encode().unwrap();
        let parsed = Frame::decode(&bytes).unwrap();
        assert_eq!(parsed, f);
    }
}

#[tokio::test]
async fn rejects_bad_magic_version_and_oversized_frame_without_allocating() {
    let mut bad_magic = Cursor::new(b"USBIP\x00\x01\x00\x00\x00".to_vec());
    assert!(matches!(
        read_frame(&mut bad_magic).await,
        Err(ProtoError::InvalidMagic)
    ));

    let mut bad_ver = Cursor::new(b"PRTC\x05\x01\x00\x00\x00\x00".to_vec());
    assert!(matches!(
        read_frame(&mut bad_ver).await,
        Err(ProtoError::UnsupportedVersion(5))
    ));

    let too_big = (MAX_FRAME_LEN + 1).to_be_bytes();
    let mut oversized_hdr = vec![b'P', b'R', b'T', b'C', 0x00, 0x10];
    oversized_hdr.extend_from_slice(&too_big);
    let mut cur = Cursor::new(oversized_hdr);
    assert!(matches!(
        read_frame(&mut cur).await,
        Err(ProtoError::FrameTooLarge(_))
    ));
}

#[tokio::test]
async fn async_write_and_read_frame_roundtrip() {
    let f = Frame::Ping { nonce: 0xDEADBEEF };
    let mut buf = Vec::new();
    write_frame(&mut buf, &f).await.unwrap();
    let mut cur = Cursor::new(buf);
    assert_eq!(read_frame(&mut cur).await.unwrap(), f);
}
