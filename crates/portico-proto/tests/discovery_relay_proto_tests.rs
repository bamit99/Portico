use portico_proto::discovery::DiscoveryPacket;
use portico_proto::relay_proto::{format_relay_uri, hash_slot_secret, parse_relay_uri, RelayFrame};
use portico_proto::ProtoError;
use std::io::Cursor;

#[test]
fn discovery_packet_roundtrip_and_invalid_rejected() {
    let req = DiscoveryPacket::Request { nonce: [1, 2, 3, 4, 5, 6, 7, 8] };
    assert_eq!(DiscoveryPacket::decode(&req.encode()).unwrap(), req);

    let resp = DiscoveryPacket::Response {
        nonce: [1, 2, 3, 4, 5, 6, 7, 8],
        tcp_port: 37420,
        tls_required: true,
        plaintext_lan: false,
        cert_fp_prefix: [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x01, 0x02],
        node_name: "OnePlus 12R".into(),
    };
    assert_eq!(DiscoveryPacket::decode(&resp.encode()).unwrap(), resp);

    assert!(matches!(
        DiscoveryPacket::decode(b"XXXX\x00\x01"),
        Err(ProtoError::InvalidMagic)
    ));
}

#[tokio::test]
async fn relay_frames_and_uri_roundtrip() {
    let node_id = [0x12u8; 16];
    let uri = format_relay_uri("127.0.0.1:37422", &node_id);
    assert_eq!(uri, "prly://127.0.0.1:37422/12121212121212121212121212121212");
    let (ep, parsed_id) = parse_relay_uri(&uri).unwrap();
    assert_eq!(ep, "127.0.0.1:37422");
    assert_eq!(parsed_id, node_id);

    assert!(matches!(
        parse_relay_uri("prly://127.0.0.1:37422/badhex"),
        Err(ProtoError::InvalidRelayUri(_))
    ));

    let frames = vec![
        RelayFrame::ControlRegister {
            node_id,
            slot_secret_hash: hash_slot_secret(b"secret"),
        },
        RelayFrame::Registered,
        RelayFrame::RegisterRejected,
        RelayFrame::ClientConnect {
            node_id,
            conn_nonce: [0x34u8; 16],
        },
        RelayFrame::IncomingClient {
            conn_nonce: [0x34u8; 16],
        },
        RelayFrame::ServerAccept {
            node_id,
            slot_secret_hash: hash_slot_secret(b"secret"),
            conn_nonce: [0x34u8; 16],
        },
        RelayFrame::AcceptRejected,
    ];
    for f in frames {
        let mut buf = Vec::new();
        f.write_to(&mut buf).await.unwrap();
        let mut cur = Cursor::new(buf);
        assert_eq!(RelayFrame::read_from(&mut cur).await.unwrap(), f);
    }
}
