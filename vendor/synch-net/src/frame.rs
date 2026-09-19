//! Length-framed postcard messages on QUIC streams (§5.1).
//!
//! Every message is a little-endian `u32` length followed by that many bytes of
//! postcard. The length is checked against [`MAX_FRAME_LEN`] before allocating.
//!
//! What that bounds, precisely: one *stream's* buffer, not a node's memory. The
//! buffer is sized from the declared length before the body is read, so a peer
//! that declares 16 MiB and sends nothing has named a 16 MiB allocation for
//! four bytes of traffic, on every stream it opens (`MAX_CONCURRENT_STREAMS`
//! of them per connection, and connections are not themselves capped). The
//! reason that
//! is not the amplification it looks like is `alloc_zeroed`: at this size it is
//! served by `mmap`, so the pages are not committed until they are written and
//! an attacker pays for only what it actually sends — measured at ~6 MB
//! resident for 1 600 such buffers. Reaching a peer at all requires a live
//! binding (`serve::serve_connection`), which is where §12 puts the answer to a
//! member behaving this way.
//!
//! The cap that matters for *work* rather than bytes is not here: it is the
//! per-field element cap on the decoded message, applied while decoding
//! ([`synch_core::MptMessage`]), because a frame this size is seconds of CPU to
//! deserialize.

use iroh::endpoint::{Connection, RecvStream, SendStream};
use serde::{de::DeserializeOwned, Serialize};
use synch_core::MAX_FRAME_LEN;

use crate::error::NetError;

/// An answer frame from a protocol that may carry an in-band refusal.
///
/// The unwrapping lives here, expressed once over both message enums, because
/// it is the piece the request/response sites had already let drift: every
/// decoded answer must pass through it before its shape is matched, or a
/// responder's stated reason is reported as a shape mismatch.
pub(crate) trait Answer: Sized {
    /// Extracts the peer's refusal, where this protocol has one.
    ///
    /// A responder that refuses says why, and the reason is the error —
    /// reading it as a shape mismatch loses the only account of what went
    /// wrong the peer will ever give.
    fn into_refusal(self) -> Result<Self, String>;
}

/// Opens a stream, sends one request frame, and closes this side.
///
/// The send half is finished immediately: every exchange under this helper is
/// one request frame followed by the peer's answer, so anything more this side
/// wrote would be a protocol error anyway.
pub(crate) async fn request<Req: Serialize>(
    connection: &Connection,
    request: &Req,
) -> Result<RecvStream, NetError> {
    let (mut send, recv) = connection.open_bi().await?;
    write_frame(&mut send, request).await?;
    let _ = send.finish();
    Ok(recv)
}

/// Reads one answer frame, unwrapping an in-band refusal into the error.
pub(crate) async fn read_answer<Ans>(recv: &mut RecvStream) -> Result<Ans, NetError>
where
    Ans: DeserializeOwned + Answer,
{
    read_frame::<Ans>(recv)
        .await?
        .into_refusal()
        .map_err(NetError::Unexpected)
}

/// One whole request/response exchange: [`request`], then [`read_answer`].
///
/// The caller keeps only the decoded answer; a protocol that reads more than
/// one frame back composes the two halves itself.
pub(crate) async fn exchange<Req, Ans>(connection: &Connection, req: &Req) -> Result<Ans, NetError>
where
    Req: Serialize,
    Ans: DeserializeOwned + Answer,
{
    let mut recv = request(connection, req).await?;
    read_answer(&mut recv).await
}

/// Writes one length-framed postcard message.
pub async fn write_frame<T: Serialize>(send: &mut SendStream, msg: &T) -> Result<(), NetError> {
    let bytes = postcard::to_stdvec(msg).map_err(|e| NetError::Encode(e.to_string()))?;
    write_bytes(send, &bytes).await
}

/// Writes one length-framed raw payload (used for bao slice bodies, §6.4).
pub(crate) async fn write_bytes(send: &mut SendStream, bytes: &[u8]) -> Result<(), NetError> {
    if bytes.len() > MAX_FRAME_LEN {
        return Err(NetError::FrameTooLarge(bytes.len()));
    }
    send.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
    send.write_all(bytes).await?;
    Ok(())
}

/// Reads one length-framed postcard message.
pub async fn read_frame<T: DeserializeOwned>(recv: &mut RecvStream) -> Result<T, NetError> {
    let bytes = read_bytes(recv).await?;
    postcard::from_bytes(&bytes).map_err(|e| NetError::Decode(e.to_string()))
}

/// Reads one length-framed raw payload.
pub(crate) async fn read_bytes(recv: &mut RecvStream) -> Result<Vec<u8>, NetError> {
    read_bounded(recv, MAX_FRAME_LEN).await
}

/// Reads one length-framed raw payload under a caller's own cap.
///
/// The cap is applied to the *declared* length, before the buffer exists, so a
/// protocol with a tighter bound than [`MAX_FRAME_LEN`] — `sync/sock/1`'s
/// `Open`, at nine kilobytes — never pays a 16 MiB allocation to find out it
/// was sent one.
pub(crate) async fn read_bounded(recv: &mut RecvStream, max: usize) -> Result<Vec<u8>, NetError> {
    let mut header = [0u8; 4];
    recv.read_exact(&mut header).await?;
    let len = u32::from_le_bytes(header) as usize;
    if len > max.min(MAX_FRAME_LEN) {
        return Err(NetError::FrameTooLarge(len));
    }
    let mut buf = vec![0u8; len];
    if len > 0 {
        recv.read_exact(&mut buf).await?;
    }
    Ok(buf)
}
