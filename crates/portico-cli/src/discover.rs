use crate::CliError;
use portico_proto::discovery::DiscoveryPacket;
use rand::RngCore;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredServer {
    pub addr: SocketAddr,
    pub tcp_port: u16,
    pub tls_required: bool,
    pub plaintext_lan: bool,
    pub cert_fp_prefix: String,
    pub node_name: String,
}

pub async fn discover_servers(
    bind_addr: SocketAddr,
    target_addr: SocketAddr,
    timeout: Duration,
) -> Result<Vec<DiscoveredServer>, CliError> {
    let sock = UdpSocket::bind(bind_addr).await?;
    let _ = sock.set_broadcast(true);

    let mut nonce = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut nonce);

    let req = DiscoveryPacket::Request { nonce }.encode();
    sock.send_to(&req, target_addr).await?;

    let mut results = Vec::new();
    let mut seen = HashSet::new();
    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 512];

    loop {
        let Some(rem) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        match tokio::time::timeout(rem, sock.recv_from(&mut buf)).await {
            Ok(Ok((n, from))) => {
                if let Ok(DiscoveryPacket::Response {
                    nonce: rnonce,
                    tcp_port,
                    tls_required,
                    plaintext_lan,
                    cert_fp_prefix,
                    node_name,
                }) = DiscoveryPacket::decode(&buf[..n])
                {
                    if rnonce == nonce && seen.insert((from.ip(), tcp_port)) {
                        results.push(DiscoveredServer {
                            addr: SocketAddr::new(from.ip(), tcp_port),
                            tcp_port,
                            tls_required,
                            plaintext_lan,
                            cert_fp_prefix: hex::encode(cert_fp_prefix),
                            node_name,
                        });
                    }
                }
            }
            Ok(Err(_)) | Err(_) => break,
        }
    }
    Ok(results)
}
