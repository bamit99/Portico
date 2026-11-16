use portico_proto::identity::NodeIdentity;
use portico_proto::relay_proto::{hash_slot_secret, RelayFrame};
use portico_proto::tls::{build_client_tls_config, build_server_tls_config, CertCheckPolicy};
use portico_relay::RelayServer;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

#[tokio::test]
async fn relay_bridges_e2e_tls_ciphertext_and_blocks_hijack() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = listener.local_addr().unwrap();
    let relay_task = tokio::spawn(async move {
        let _ = RelayServer::run(listener).await;
    });

    let server_id = NodeIdentity::generate("portico-node").unwrap();
    let node_id = server_id.node_id();
    let secret_hash = hash_slot_secret(b"my-slot-secret");
    let wrong_hash = hash_slot_secret(b"wrong-secret");

    // 1. Server registers control slot
    let mut srv_ctrl = TcpStream::connect(relay_addr).await.unwrap();
    RelayFrame::ControlRegister {
        node_id,
        slot_secret_hash: secret_hash,
    }
    .write_to(&mut srv_ctrl)
    .await
    .unwrap();
    assert_eq!(
        RelayFrame::read_from(&mut srv_ctrl).await.unwrap(),
        RelayFrame::Registered
    );

    // 2. Client connects to relay
    let mut cli_tcp = TcpStream::connect(relay_addr).await.unwrap();
    let conn_nonce = [0xABu8; 16];
    RelayFrame::ClientConnect {
        node_id,
        conn_nonce,
    }
    .write_to(&mut cli_tcp)
    .await
    .unwrap();

    // 3. Server receives IncomingClient
    assert_eq!(
        RelayFrame::read_from(&mut srv_ctrl).await.unwrap(),
        RelayFrame::IncomingClient { conn_nonce }
    );

    // 4. Attacker attempts ServerAccept with wrong secret_hash -> rejected
    let mut evil_data = TcpStream::connect(relay_addr).await.unwrap();
    RelayFrame::ServerAccept {
        node_id,
        slot_secret_hash: wrong_hash,
        conn_nonce,
    }
    .write_to(&mut evil_data)
    .await
    .unwrap();
    assert_eq!(
        RelayFrame::read_from(&mut evil_data).await.unwrap(),
        RelayFrame::AcceptRejected
    );

    // 5. Real server opens data connection and both sides perform E2E TLS 1.3 handshake
    let mut srv_data = TcpStream::connect(relay_addr).await.unwrap();
    RelayFrame::ServerAccept {
        node_id,
        slot_secret_hash: secret_hash,
        conn_nonce,
    }
    .write_to(&mut srv_data)
    .await
    .unwrap();

    let srv_tls_cfg = build_server_tls_config(&server_id, None).unwrap();
    let acceptor = TlsAcceptor::from(srv_tls_cfg);
    let (cli_tls_cfg, _) = build_client_tls_config(
        None,
        CertCheckPolicy::PinnedOrFirstUse {
            expected_fp: Some(server_id.fingerprint_hex()),
            trust_first_use: false,
        },
    )
    .unwrap();
    let connector = TlsConnector::from(cli_tls_cfg);

    let (srv_res, cli_res) = tokio::join!(
        acceptor.accept(srv_data),
        connector.connect("localhost".try_into().unwrap(), cli_tcp)
    );
    let mut srv_tls = srv_res.unwrap();
    let mut cli_tls = cli_res.unwrap();

    cli_tls.write_all(b"secret-usb-payload").await.unwrap();
    let mut buf = [0u8; 18];
    srv_tls.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"secret-usb-payload");

    relay_task.abort();
}
