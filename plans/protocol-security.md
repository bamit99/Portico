# Protocol & Security (`portico-proto` + `portico-relay`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Portico's versioned wire protocol (`PRTC` v0), TLS 1.3 + TOFU pinning + reverse mTLS security primitives, constant-time claim-token auth, `PRDS` v0 LAN discovery frames, and `PRLY` v0 ciphertext-only relay server/client.

**Architecture:** `crates/portico-proto` is a pure-Rust library crate (no USB hardware dependencies) shared by the core daemon, Android JNI bridge, desktop CLI, and relay server. `crates/portico-relay` depends only on `portico-proto` and `tokio` to bridge client and server TCP streams using opaque `node_id` and `conn_nonce` tokens while seeing only TLS ciphertext.

**Tech Stack:** Rust 2024/2021 edition, `tokio`, `rustls` (ring crypto provider), `tokio-rustls`, `rcgen`, `sha2`, `hex`, `subtle`, `rand`, `serde`, `serde_json`, `thiserror`.

**Spec:** `DESIGN.md` (§6, §7, §8), `FEATURES.md` (§Networking, §Security), `README.md` (§Appendix clean-room boundary).

## Global Constraints

- Portico TCP port default: `37420`; UDP discovery port: `37421`; Relay TCP port default: `37422`.
- Wire protocol magic: `b"PRTC"`, version: `0x00`, max payload length: `4_194_304` bytes (4 MiB).
- TLS 1.3 required by default; plaintext allowed ONLY when explicitly opted in AND peer IP is loopback/RFC1918/link-local (`127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16`, `::1`, `fc00::/7`, `fe80::/10`).
- Reverse clients require mutual TLS (both endpoints pin peer cert SHA-256).
- Relay sees ciphertext only: zero tokens, cert private keys, or USB descriptors in `PRLY` framing.
- Claim token comparison MUST be constant-time (`subtle::ConstantTimeEq`); token never logged.
- Zero GPL code from look-don't-link repositories.

## Review Focus

1. Truncated, oversized (`> 4 MiB`), or corrupt-magic `PRTC`/`PRLY`/`PRDS` frames causing OOM allocation or panic instead of typed `ProtoError`. (Pinned in Task 2)
2. Plaintext connection attempt from/to a public routable IP even when `plaintext_lan` is enabled — must be unconditionally rejected. (Pinned in Task 4)
3. TOFU certificate pin mismatch or uppercase/colon-formatted fingerprint input bypassing verification — must normalize and fail closed before sending `ClientHello`. (Pinned in Task 3)
4. Reverse-client mTLS connecting without matching client cert pin or accepting arbitrary client certs — must abort TLS handshake. (Pinned in Task 4)
5. Relay `ServerAccept` spoofing a `node_id` without matching `SHA-256(slot_secret)` — must reject hijack and preserve active server slot. (Pinned in Task 6)

---

### Task 1: Workspace Scaffold & `portico-proto` Error / Constants / Auth Token

**Files:**
- Create: `Cargo.toml`
- Create: `crates/portico-proto/Cargo.toml`
- Create: `crates/portico-proto/src/lib.rs`
- Create: `crates/portico-proto/src/token.rs`
- Test: `crates/portico-proto/tests/token_tests.rs`

**Interfaces:**
- Produces:
  - `portico_proto::DEFAULT_PORT: u16 = 37420`
  - `portico_proto::DEFAULT_DISCOVERY_PORT: u16 = 37421`
  - `portico_proto::DEFAULT_RELAY_PORT: u16 = 37422`
  - `portico_proto::MAX_FRAME_LEN: u32 = 4_194_304`
  - `portico_proto::ProtoError`
  - `portico_proto::token::{generate_claim_token() -> String, verify_claim_token(expected: &str, presented: &[u8]) -> bool, redact_token(s: &str) -> String}`

- [ ] **Step 1: Create workspace `Cargo.toml` and `crates/portico-proto/Cargo.toml` + `src/lib.rs` stub**
  Include workspace members `crates/portico-proto`, `crates/portico-core`, `crates/portico-cli`, `crates/portico-relay` (we will add `crates/portico-jni` in Android plan). Declare `token` module stub returning `todo!()`.

- [ ] **Step 2: Write failing tests in `crates/portico-proto/tests/token_tests.rs`**
  Test:
  - `generated_token_is_24_chars_alnum_and_unique`: two calls to `generate_claim_token()` produce 24-char `[A-Z2-7]`/`[A-Za-z0-9]` strings that differ.
  - `verify_claim_token_matches_exact_and_rejects_wrong_or_empty`: exact match returns `true`; wrong last char, prefix, longer slice, and empty slice return `false`.
  - `redact_token_masks_secret`: `"ABCD1234EFGH5678IJKL9012"` redacts without leaking middle characters.

- [ ] **Step 3: Run tests to verify RED**
  Run: `cargo test -p portico-proto --test token_tests`
  Expected: FAIL (`not yet implemented` / compile failure on stub).

- [ ] **Step 4: Implement `crates/portico-proto/src/token.rs` and `ProtoError` in `src/lib.rs`**
  Use `rand::rngs::OsRng` for 24-char token generation (`Crockford base32` or uppercase alphanumeric without ambiguous `0/O/1/I`) and `subtle::ConstantTimeEq` for `verify_claim_token`.

- [ ] **Step 5: Run tests to verify GREEN**
  Run: `cargo test -p portico-proto`
  Expected: PASS (3/3 tests).

- [ ] **Step 6: Commit**
  `git add Cargo.toml crates/portico-proto && git commit -m "feat(proto): workspace scaffold and constant-time claim token"`

---

### Task 2: `PRTC` v0 Binary Frame Codec & Message Types

**Files:**
- Create: `crates/portico-proto/src/frame.rs`
- Modify: `crates/portico-proto/src/lib.rs`
- Test: `crates/portico-proto/tests/frame_tests.rs`

**Interfaces:**
- Consumes: `portico_proto::ProtoError`, `MAX_FRAME_LEN`
- Produces:
  - `portico_proto::frame::{PROTOCOL_MAGIC, PROTOCOL_VERSION_V0, AuthKind, HelloStatus, TunnelStatus, LicenseTier, DeviceSummary, Frame}`
  - `Frame::encode(&self) -> Result<Vec<u8>, ProtoError>`
  - `Frame::decode(buf: &[u8]) -> Result<Frame, ProtoError>`
  - `async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, frame: &Frame) -> Result<(), ProtoError>`
  - `async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Frame, ProtoError>`

- [ ] **Step 1: Write failing tests in `crates/portico-proto/tests/frame_tests.rs`**
  Test:
  - Hand-derived byte fixture for `ClientHello` (`b"PRTC\x00\x01..."` with `min_ver=0, max_ver=0, auth_kind=1, credentials=b"TOK", client_name="laptop", capabilities=1`) decodes to exact struct and re-encodes identically.
  - Roundtrip for every variant: `ServerHello`, `ListDevicesReq`, `ListDevicesResp`, `OpenTunnelReq`, `OpenTunnelResp`, `RequestReverseDataConn`, `ReverseDataHello`, `Ping`, `Pong`, `Error`.
  - Rejects bad magic (`b"USBIP..."` -> `ProtoError::InvalidMagic`), unsupported version (`0x05` -> `ProtoError::UnsupportedVersion(5)`), oversized header length (`4_194_305` -> `ProtoError::FrameTooLarge`) BEFORE allocating payload, and truncated EOF (`UnexpectedEof`).

- [ ] **Step 2: Run tests to verify RED**
  Run: `cargo test -p portico-proto --test frame_tests`
  Expected: FAIL.

- [ ] **Step 3: Implement `crates/portico-proto/src/frame.rs`**
  Enforce `len <= MAX_FRAME_LEN` prior to `vec![0u8; len as usize]` allocation. Enforce bounds on string `u16` reads inside payload cursor.

- [ ] **Step 4: Run tests to verify GREEN**
  Run: `cargo test -p portico-proto`
  Expected: PASS.

- [ ] **Step 5: Commit**
  `git add crates/portico-proto && git commit -m "feat(proto): PRTC v0 binary framing and message codec"`

---

### Task 3: Identity Cert Generation, SHA-256 Fingerprint, and TOFU `KnownHosts` Store

**Files:**
- Create: `crates/portico-proto/src/identity.rs`
- Create: `crates/portico-proto/src/tofu.rs`
- Modify: `crates/portico-proto/src/lib.rs`
- Test: `crates/portico-proto/tests/identity_tofu_tests.rs`

**Interfaces:**
- Produces:
  - `NodeIdentity { cert_der: Vec<u8>, key_der: Vec<u8> }`
  - `NodeIdentity::generate(common_name: &str) -> Result<Self, ProtoError>`
  - `NodeIdentity::load_or_generate(dir: &Path, prefix: &str, common_name: &str) -> Result<Self, ProtoError>`
  - `NodeIdentity::fingerprint_hex(&self) -> String` (64 lowercase hex)
  - `NodeIdentity::node_id(&self) -> [u8; 16]` (`SHA-256(b"portico-node-v0" || cert_der)[0..16]`)
  - `cert_fingerprint_hex(der: &[u8]) -> String`
  - `format_short_fingerprint(fp_hex: &str) -> String` (`XXXX-XXXX-XXXX-XXXX`)
  - `normalize_fingerprint(input: &str) -> Result<String, ProtoError>` (strips `:`, `-`, whitespace, lowercases, checks 64 hex chars)
  - `KnownHosts { entries: BTreeMap<String, String> }` with `load(path)`, `save_atomic(path)`, `verify_or_insert(endpoint, fp)` returning `TofuDecision::{Trusted, FirstUse(String), Mismatch { expected, actual }}`

- [ ] **Step 1: Write failing tests in `crates/portico-proto/tests/identity_tofu_tests.rs`**
  Test:
  - `load_or_generate` persists `cert.der` + `key.der` (mode `0600` on Unix) and reloads identical fingerprint & `node_id` on second call.
  - `normalize_fingerprint` accepts `"A4:F2:..."` / `"a4f2-..."` and rejects invalid length or non-hex chars.
  - `KnownHosts::check` returns `FirstUse` on unknown endpoint, `Trusted` after pinning, `Mismatch` when a different cert fingerprint is presented.

- [ ] **Step 2: Run tests to verify RED**
  Run: `cargo test -p portico-proto --test identity_tofu_tests`
  Expected: FAIL.

- [ ] **Step 3: Implement `identity.rs` and `tofu.rs`**
  Use `rcgen` (ECDSA P-256 self-signed cert) + `sha2::Sha256`. Use temp-file + `sync_all` + `rename` with `0o600` permissions on Unix for key/known_hosts persistence.

- [ ] **Step 4: Run tests to verify GREEN**
  Run: `cargo test -p portico-proto`
  Expected: PASS.

- [ ] **Step 5: Commit**
  `git add crates/portico-proto && git commit -m "feat(proto): ECDSA node identity generation and TOFU known_hosts store"`

---

### Task 4: TLS 1.3 Server/Client Config, TOFU Verifier, Mutual TLS Verifiers, and LAN IP Guard

**Files:**
- Create: `crates/portico-proto/src/tls.rs`
- Modify: `crates/portico-proto/src/lib.rs`
- Test: `crates/portico-proto/tests/tls_tests.rs`

**Interfaces:**
- Consumes: `NodeIdentity`, `cert_fingerprint_hex`, `normalize_fingerprint`
- Produces:
  - `is_lan_or_loopback_ip(ip: IpAddr) -> bool`
  - `build_server_tls_config(identity: &NodeIdentity, client_pin_verifier: Option<Arc<PinnedClientCertVerifier>>) -> Result<Arc<rustls::ServerConfig>, ProtoError>`
  - `build_client_tls_config_tofu(identity: Option<&NodeIdentity>, expected_fp: Option<&str>, tofu_callback: ...) -> Result<(Arc<rustls::ClientConfig>, Arc<ObservedFingerprint>), ProtoError>`
  - `PinnedServerCertVerifier`, `PinnedClientCertVerifier` (accepts dynamic allowlist of lowercase SHA-256 fingerprints)

- [ ] **Step 1: Write failing tests in `crates/portico-proto/tests/tls_tests.rs`**
  Test:
  - `is_lan_or_loopback_ip`: `127.0.0.1`, `10.0.0.5`, `172.16.5.4`, `192.168.1.42`, `169.254.1.1`, `::1`, `fd12::1`, `fe80::1` -> `true`; `8.8.8.8`, `1.1.1.1`, `172.32.0.1`, `2001:4860:4860::8888` -> `false`.
  - Real `tokio-rustls` loopback handshake:
    - TOFU with matching pin succeeds and records server fingerprint.
    - TOFU with wrong pin fails handshake before any application bytes are exchanged.
    - mTLS with allowed client cert succeeds; mTLS with unpinned client cert fails handshake.

- [ ] **Step 2: Run tests to verify RED**
  Run: `cargo test -p portico-proto --test tls_tests`
  Expected: FAIL.

- [ ] **Step 3: Implement `crates/portico-proto/src/tls.rs`**
  Note: Explicitly install `rustls::crypto::ring::default_provider().install_default()` once via `std::sync::Once` inside `tls::init_crypto_provider()` so `rustls` 0.23 never panics on missing default provider! Enforce TLS 1.3 (`&rustls::version::TLS13` or `with_protocol_versions(&[&rustls::version::TLS13])`).

- [ ] **Step 4: Run tests to verify GREEN**
  Run: `cargo test -p portico-proto`
  Expected: PASS.

- [ ] **Step 5: Commit**
  `git add crates/portico-proto && git commit -m "feat(proto): TLS 1.3 configs, TOFU/mTLS cert verifiers, and LAN IP guard"`

---

### Task 5: `PRDS` v0 LAN Discovery & `PRLY` v0 Relay Framing + `portico-relay` Server

**Files:**
- Create: `crates/portico-proto/src/discovery.rs`
- Create: `crates/portico-proto/src/relay_proto.rs`
- Modify: `crates/portico-proto/src/lib.rs`
- Create: `crates/portico-relay/Cargo.toml`
- Create: `crates/portico-relay/src/lib.rs`
- Test: `crates/portico-proto/tests/discovery_relay_proto_tests.rs`
- Test: `crates/portico-relay/tests/relay_e2e_tests.rs`

**Interfaces:**
- Produces in `portico-proto`:
  - `DiscoveryPacket::{Request { nonce: [u8; 8] }, Response { nonce: [u8; 8], tcp_port: u16, tls_required: bool, plaintext_lan: bool, cert_fp_prefix: [u8; 8], node_name: String }}`
  - `DiscoveryPacket::encode(&self) -> Vec<u8>`, `DiscoveryPacket::decode(bytes: &[u8]) -> Result<Self, ProtoError>`
  - `RelayFrame::{ControlRegister { node_id: [u8; 16], slot_secret_hash: [u8; 32] }, Registered, RegisterRejected, ClientConnect { node_id: [u8; 16], conn_nonce: [u8; 16] }, IncomingClient { conn_nonce: [u8; 16] }, ServerAccept { node_id: [u8; 16], slot_secret_hash: [u8; 32], conn_nonce: [u8; 16] }}`
  - `parse_relay_uri(uri: &str) -> Result<(String, [u8; 16]), ProtoError>` (`prly://host:port/<32-hex-node-id>`)
- Produces in `portico-relay`:
  - `RelayServer::run(listener: TcpListener) -> std::io::Result<()>`

- [ ] **Step 1: Write failing tests for `discovery`, `relay_proto`, and `portico-relay`**
  Test:
  - `DiscoveryPacket` roundtrips `Request` and `Response`, rejects wrong magic or truncated name.
  - `parse_relay_uri` parses `prly://relay.example:37422/00112233445566778899aabbccddeeff` and rejects malformed hex/missing node_id.
  - `RelayServer` E2E test: server registers `node_id` + `slot_secret_hash`, attacker tries `ServerAccept` with wrong `slot_secret_hash` (rejected!), real client sends `ClientConnect`, server receives `IncomingClient(nonce)` and sends `ServerAccept`, then both sides run TLS 1.3 `tokio-rustls` handshake + echo payload through the relay and assert the relay never saw plaintext bytes.

- [ ] **Step 2: Run tests to verify RED**
  Run: `cargo test -p portico-proto --test discovery_relay_proto_tests && cargo test -p portico-relay`
  Expected: FAIL.

- [ ] **Step 3: Implement `discovery.rs`, `relay_proto.rs`, and `crates/portico-relay/src/lib.rs`**

- [ ] **Step 4: Run tests to verify GREEN**
  Run: `cargo test -p portico-proto -p portico-relay`
  Expected: PASS.

- [ ] **Step 5: Commit**
  `git add crates/portico-proto crates/portico-relay && git commit -m "feat(proto,relay): LAN discovery frames and ciphertext-only relay server"`
