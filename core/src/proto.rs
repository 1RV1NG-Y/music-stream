//! Wire messages and pairing tickets.

use anyhow::{Context, Result, bail};
use iroh::EndpointAddr;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const SYNC_ALPN: &[u8] = b"music/sync/1";
pub const PAIR_ALPN: &[u8] = b"music/pair/1";
/// Bumped whenever messages or document layout change incompatibly.
pub const PROTOCOL_VERSION: u32 = 1;
const MAX_FRAME: usize = 128 * 1024 * 1024;
const TICKET_PREFIX: &str = "MUSIC1";

#[derive(Debug, Serialize, Deserialize)]
pub enum SyncMsg {
    /// First message on each side. `vv` is the sender's durable version.
    Hello { protocol: u32, library: String, vv: Vec<u8> },
    /// Loro updates. `vv` is a version the sender holds in full, so the receiver
    /// knows the sender already has everything up to it.
    Updates { data: Vec<u8>, vv: Vec<u8> },
    /// Sent after received updates are durable. `vv` is the receiver's durable version.
    Ack { vv: Vec<u8> },
    Reject { reason: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub enum PairMsg {
    /// Scanner of a `Join` code asks to join the code owner's library.
    Request { secret: [u8; 32], name: String, platform: String },
    /// Scanner of an `Add` code brings the code owner into the scanner's library.
    Offer { secret: [u8; 32], library: String, name: String, platform: String },
    Accepted { library: String, name: String, platform: String },
    Rejected { reason: String },
}

pub async fn write_msg<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, msg: &T) -> Result<()> {
    let bytes = postcard::to_stdvec(msg)?;
    w.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

/// Returns `None` on a clean end of stream between messages.
pub async fn read_msg<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        bail!("message of {len} bytes is too large");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(Some(postcard::from_bytes(&buf).context("malformed message")?))
}

/// What a pairing code means for the device that scans it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TicketKind {
    /// Shown by a set-up device: the scanner joins its library.
    Join,
    /// Shown by a new device that can't scan (e.g. a PC): the scanner adds it to its library.
    Add,
}

/// What a QR code or pasted invitation carries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticket {
    pub kind: TicketKind,
    pub addr: EndpointAddr,
    pub secret: [u8; 32],
}

impl Ticket {
    /// Uppercase base32 so the QR code can use its compact alphanumeric mode.
    pub fn encode(&self) -> String {
        let bytes = postcard::to_stdvec(self).expect("ticket serializes");
        format!("{TICKET_PREFIX}{}", data_encoding::BASE32_NOPAD.encode(&bytes))
    }

    pub fn decode(text: &str) -> Result<Self> {
        let cleaned: String = text.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_uppercase();
        let body = cleaned.strip_prefix(TICKET_PREFIX).context("This isn't a music-stream invitation.")?;
        let bytes = data_encoding::BASE32_NOPAD
            .decode(body.as_bytes())
            .ok()
            .context("This invitation is incomplete or mistyped.")?;
        postcard::from_bytes(&bytes).ok().context("This invitation is incomplete or mistyped.")
    }
}

pub fn qr_svg(text: &str) -> Result<String> {
    use qrcode::{EcLevel, QrCode, render::svg};
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M)?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(240, 240)
        .dark_color(svg::Color("#111"))
        .light_color(svg::Color("#fff"))
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_round_trip() {
        let key = iroh::SecretKey::generate();
        let addr = EndpointAddr::new(key.public()).with_ip_addr("192.168.1.20:47120".parse().unwrap());
        let ticket = Ticket { kind: TicketKind::Add, addr, secret: [7; 32] };
        let text = ticket.encode();
        assert!(text.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
        let back = Ticket::decode(&format!("  {}\n", text.to_lowercase())).unwrap();
        assert_eq!(back.addr, ticket.addr);
        assert_eq!(back.secret, ticket.secret);
        assert_eq!(back.kind, TicketKind::Add);
        assert!(Ticket::decode("hello").is_err());
        assert!(qr_svg(&text).unwrap().starts_with("<?xml"));
    }

    #[tokio::test]
    async fn framing_round_trip() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        write_msg(&mut a, &SyncMsg::Ack { vv: vec![1, 2, 3] }).await.unwrap();
        drop(a);
        match read_msg::<_, SyncMsg>(&mut b).await.unwrap() {
            Some(SyncMsg::Ack { vv }) => assert_eq!(vv, vec![1, 2, 3]),
            other => panic!("{other:?}"),
        }
        assert!(read_msg::<_, SyncMsg>(&mut b).await.unwrap().is_none());
    }
}
