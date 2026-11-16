use crate::{ProtoError, MAX_FRAME_LEN};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_MAGIC: &[u8; 4] = b"PRTC";
pub const PROTOCOL_VERSION_V0: u8 = 0x00;
pub const CAP_USBIP_V1: u32 = 1 << 0;

const MSG_CLIENT_HELLO: u8 = 0x01;
const MSG_SERVER_HELLO: u8 = 0x02;
const MSG_LIST_DEVICES_REQ: u8 = 0x10;
const MSG_LIST_DEVICES_RESP: u8 = 0x11;
const MSG_OPEN_TUNNEL_REQ: u8 = 0x20;
const MSG_OPEN_TUNNEL_RESP: u8 = 0x21;
const MSG_REQUEST_REVERSE_DATA_CONN: u8 = 0x22;
const MSG_REVERSE_DATA_HELLO: u8 = 0x23;
const MSG_PING: u8 = 0x30;
const MSG_PONG: u8 = 0x31;
const MSG_ERROR: u8 = 0x7F;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    ClaimToken = 0x01,
    AccountCredentials = 0x02,
}

impl AuthKind {
    fn from_u8(v: u8) -> Result<Self, ProtoError> {
        match v {
            0x01 => Ok(Self::ClaimToken),
            0x02 => Ok(Self::AccountCredentials),
            other => Err(ProtoError::MalformedPayload(format!(
                "invalid auth_kind {other:#04x}"
            ))),
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelloStatus {
    Ok = 0x00,
    AuthFailed = 0x01,
    UnsupportedVersion = 0x02,
    PolicyRejected = 0x03,
}

impl HelloStatus {
    fn from_u8(v: u8) -> Result<Self, ProtoError> {
        match v {
            0x00 => Ok(Self::Ok),
            0x01 => Ok(Self::AuthFailed),
            0x02 => Ok(Self::UnsupportedVersion),
            0x03 => Ok(Self::PolicyRejected),
            other => Err(ProtoError::MalformedPayload(format!(
                "invalid hello status {other:#04x}"
            ))),
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelStatus {
    Ok = 0x00,
    NotFound = 0x01,
    InUse = 0x02,
    Ignored = 0x03,
}

impl TunnelStatus {
    fn from_u8(v: u8) -> Result<Self, ProtoError> {
        match v {
            0x00 => Ok(Self::Ok),
            0x01 => Ok(Self::NotFound),
            0x02 => Ok(Self::InUse),
            0x03 => Ok(Self::Ignored),
            other => Err(ProtoError::MalformedPayload(format!(
                "invalid tunnel status {other:#04x}"
            ))),
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenseTier {
    Free = 0x00,
}

impl LicenseTier {
    fn from_u8(v: u8) -> Result<Self, ProtoError> {
        match v {
            0x00 => Ok(Self::Free),
            other => Err(ProtoError::MalformedPayload(format!(
                "invalid license tier {other:#04x}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSummary {
    pub bus_id: String,
    pub vid: u16,
    pub pid: u16,
    pub device_class: u8,
    pub device_subclass: u8,
    pub device_protocol: u8,
    pub speed: u32,
    pub manufacturer: String,
    pub product: String,
    pub serial: String,
    pub in_use: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    ClientHello {
        min_ver: u8,
        max_ver: u8,
        auth_kind: AuthKind,
        credentials: Vec<u8>,
        client_name: String,
        capabilities: u32,
    },
    ServerHello {
        chosen_ver: u8,
        status: HelloStatus,
        server_name: String,
        capabilities: u32,
        tier: LicenseTier,
        message: String,
    },
    ListDevicesReq,
    ListDevicesResp {
        devices: Vec<DeviceSummary>,
    },
    OpenTunnelReq {
        bus_id: String,
    },
    OpenTunnelResp {
        status: TunnelStatus,
        message: String,
    },
    RequestReverseDataConn {
        conn_nonce: [u8; 16],
        bus_id: String,
    },
    ReverseDataHello {
        conn_nonce: [u8; 16],
    },
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
    Error {
        code: u8,
        message: String,
    },
}

fn push_bytes(out: &mut Vec<u8>, b: &[u8]) -> Result<(), ProtoError> {
    let len = u16::try_from(b.len())
        .map_err(|_| ProtoError::MalformedPayload("field exceeds u16 length".into()))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(b);
    Ok(())
}

fn push_str(out: &mut Vec<u8>, s: &str) -> Result<(), ProtoError> {
    push_bytes(out, s.as_bytes())
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn read_exact(&mut self, n: usize) -> Result<&'a [u8], ProtoError> {
        if self.pos + n > self.buf.len() {
            return Err(ProtoError::MalformedPayload("unexpected end of payload".into()));
        }
        let slice = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, ProtoError> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, ProtoError> {
        let b = self.read_exact(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn read_u32(&mut self) -> Result<u32, ProtoError> {
        let b = self.read_exact(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_u64(&mut self) -> Result<u64, ProtoError> {
        let b = self.read_exact(8)?;
        Ok(u64::from_be_bytes(b.try_into().unwrap()))
    }

    fn read_bytes(&mut self) -> Result<Vec<u8>, ProtoError> {
        let len = self.read_u16()? as usize;
        Ok(self.read_exact(len)?.to_vec())
    }

    fn read_str(&mut self) -> Result<String, ProtoError> {
        let len = self.read_u16()? as usize;
        let b = self.read_exact(len)?;
        String::from_utf8(b.to_vec())
            .map_err(|_| ProtoError::MalformedPayload("invalid UTF-8 string".into()))
    }

    fn ensure_exhausted(&self) -> Result<(), ProtoError> {
        if self.pos != self.buf.len() {
            return Err(ProtoError::MalformedPayload("trailing bytes in frame".into()));
        }
        Ok(())
    }
}

impl Frame {
    fn encode_body(&self) -> Result<(u8, Vec<u8>), ProtoError> {
        let mut p = Vec::new();
        let msg_type = match self {
            Frame::ClientHello {
                min_ver,
                max_ver,
                auth_kind,
                credentials,
                client_name,
                capabilities,
            } => {
                p.push(*min_ver);
                p.push(*max_ver);
                p.push(*auth_kind as u8);
                push_bytes(&mut p, credentials)?;
                push_str(&mut p, client_name)?;
                p.extend_from_slice(&capabilities.to_be_bytes());
                MSG_CLIENT_HELLO
            }
            Frame::ServerHello {
                chosen_ver,
                status,
                server_name,
                capabilities,
                tier,
                message,
            } => {
                p.push(*chosen_ver);
                p.push(*status as u8);
                push_str(&mut p, server_name)?;
                p.extend_from_slice(&capabilities.to_be_bytes());
                p.push(*tier as u8);
                push_str(&mut p, message)?;
                MSG_SERVER_HELLO
            }
            Frame::ListDevicesReq => MSG_LIST_DEVICES_REQ,
            Frame::ListDevicesResp { devices } => {
                let count = u16::try_from(devices.len())
                    .map_err(|_| ProtoError::MalformedPayload("too many devices".into()))?;
                p.extend_from_slice(&count.to_be_bytes());
                for d in devices {
                    push_str(&mut p, &d.bus_id)?;
                    p.extend_from_slice(&d.vid.to_be_bytes());
                    p.extend_from_slice(&d.pid.to_be_bytes());
                    p.push(d.device_class);
                    p.push(d.device_subclass);
                    p.push(d.device_protocol);
                    p.extend_from_slice(&d.speed.to_be_bytes());
                    push_str(&mut p, &d.manufacturer)?;
                    push_str(&mut p, &d.product)?;
                    push_str(&mut p, &d.serial)?;
                    p.push(u8::from(d.in_use));
                }
                MSG_LIST_DEVICES_RESP
            }
            Frame::OpenTunnelReq { bus_id } => {
                push_str(&mut p, bus_id)?;
                MSG_OPEN_TUNNEL_REQ
            }
            Frame::OpenTunnelResp { status, message } => {
                p.push(*status as u8);
                push_str(&mut p, message)?;
                MSG_OPEN_TUNNEL_RESP
            }
            Frame::RequestReverseDataConn { conn_nonce, bus_id } => {
                p.extend_from_slice(conn_nonce);
                push_str(&mut p, bus_id)?;
                MSG_REQUEST_REVERSE_DATA_CONN
            }
            Frame::ReverseDataHello { conn_nonce } => {
                p.extend_from_slice(conn_nonce);
                MSG_REVERSE_DATA_HELLO
            }
            Frame::Ping { nonce } => {
                p.extend_from_slice(&nonce.to_be_bytes());
                MSG_PING
            }
            Frame::Pong { nonce } => {
                p.extend_from_slice(&nonce.to_be_bytes());
                MSG_PONG
            }
            Frame::Error { code, message } => {
                p.push(*code);
                push_str(&mut p, message)?;
                MSG_ERROR
            }
        };
        Ok((msg_type, p))
    }

    pub fn encode(&self) -> Result<Vec<u8>, ProtoError> {
        let (msg_type, payload) = self.encode_body()?;
        let len = u32::try_from(payload.len())
            .map_err(|_| ProtoError::FrameTooLarge(u32::MAX))?;
        if len > MAX_FRAME_LEN {
            return Err(ProtoError::FrameTooLarge(len));
        }
        let mut out = Vec::with_capacity(10 + payload.len());
        out.extend_from_slice(PROTOCOL_MAGIC);
        out.push(PROTOCOL_VERSION_V0);
        out.push(msg_type);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&payload);
        Ok(out)
    }

    fn decode_payload(msg_type: u8, payload: &[u8]) -> Result<Self, ProtoError> {
        let mut r = Reader::new(payload);
        let frame = match msg_type {
            MSG_CLIENT_HELLO => {
                let min_ver = r.read_u8()?;
                let max_ver = r.read_u8()?;
                let auth_kind = AuthKind::from_u8(r.read_u8()?)?;
                let credentials = r.read_bytes()?;
                let client_name = r.read_str()?;
                let capabilities = r.read_u32()?;
                Frame::ClientHello {
                    min_ver,
                    max_ver,
                    auth_kind,
                    credentials,
                    client_name,
                    capabilities,
                }
            }
            MSG_SERVER_HELLO => {
                let chosen_ver = r.read_u8()?;
                let status = HelloStatus::from_u8(r.read_u8()?)?;
                let server_name = r.read_str()?;
                let capabilities = r.read_u32()?;
                let tier = LicenseTier::from_u8(r.read_u8()?)?;
                let message = r.read_str()?;
                Frame::ServerHello {
                    chosen_ver,
                    status,
                    server_name,
                    capabilities,
                    tier,
                    message,
                }
            }
            MSG_LIST_DEVICES_REQ => Frame::ListDevicesReq,
            MSG_LIST_DEVICES_RESP => {
                let count = r.read_u16()? as usize;
                let mut devices = Vec::with_capacity(count.min(1024));
                for _ in 0..count {
                    devices.push(DeviceSummary {
                        bus_id: r.read_str()?,
                        vid: r.read_u16()?,
                        pid: r.read_u16()?,
                        device_class: r.read_u8()?,
                        device_subclass: r.read_u8()?,
                        device_protocol: r.read_u8()?,
                        speed: r.read_u32()?,
                        manufacturer: r.read_str()?,
                        product: r.read_str()?,
                        serial: r.read_str()?,
                        in_use: r.read_u8()? != 0,
                    });
                }
                Frame::ListDevicesResp { devices }
            }
            MSG_OPEN_TUNNEL_REQ => {
                let bus_id = r.read_str()?;
                Frame::OpenTunnelReq { bus_id }
            }
            MSG_OPEN_TUNNEL_RESP => {
                let status = TunnelStatus::from_u8(r.read_u8()?)?;
                let message = r.read_str()?;
                Frame::OpenTunnelResp { status, message }
            }
            MSG_REQUEST_REVERSE_DATA_CONN => {
                let conn_nonce: [u8; 16] = r.read_exact(16)?.try_into().unwrap();
                let bus_id = r.read_str()?;
                Frame::RequestReverseDataConn { conn_nonce, bus_id }
            }
            MSG_REVERSE_DATA_HELLO => {
                let conn_nonce: [u8; 16] = r.read_exact(16)?.try_into().unwrap();
                Frame::ReverseDataHello { conn_nonce }
            }
            MSG_PING => Frame::Ping {
                nonce: r.read_u64()?,
            },
            MSG_PONG => Frame::Pong {
                nonce: r.read_u64()?,
            },
            MSG_ERROR => {
                let code = r.read_u8()?;
                let message = r.read_str()?;
                Frame::Error { code, message }
            }
            other => return Err(ProtoError::UnknownMessageType(other)),
        };
        r.ensure_exhausted()?;
        Ok(frame)
    }

    pub fn decode(buf: &[u8]) -> Result<Self, ProtoError> {
        if buf.len() < 10 {
            return Err(ProtoError::MalformedPayload("frame shorter than 10B header".into()));
        }
        let (msg_type, len) = parse_header(&buf[..10])?;
        if buf.len() != 10 + len as usize {
            return Err(ProtoError::MalformedPayload(
                "frame buffer length mismatch".into(),
            ));
        }
        Self::decode_payload(msg_type, &buf[10..])
    }
}

fn parse_header(hdr: &[u8]) -> Result<(u8, u32), ProtoError> {
    if &hdr[0..4] != PROTOCOL_MAGIC {
        return Err(ProtoError::InvalidMagic);
    }
    let ver = hdr[4];
    if ver != PROTOCOL_VERSION_V0 {
        return Err(ProtoError::UnsupportedVersion(ver));
    }
    let msg_type = hdr[5];
    let len = u32::from_be_bytes([hdr[6], hdr[7], hdr[8], hdr[9]]);
    if len > MAX_FRAME_LEN {
        return Err(ProtoError::FrameTooLarge(len));
    }
    Ok((msg_type, len))
}

pub async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    frame: &Frame,
) -> Result<(), ProtoError> {
    let bytes = frame.encode()?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Frame, ProtoError> {
    let mut hdr = [0u8; 10];
    r.read_exact(&mut hdr).await?;
    let (msg_type, len) = parse_header(&hdr)?;
    let mut payload = vec![0u8; len as usize];
    if len > 0 {
        r.read_exact(&mut payload).await?;
    }
    Frame::decode_payload(msg_type, &payload)
}
