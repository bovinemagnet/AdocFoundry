//! Frames over a stream: the length is checked before anything is allocated, and before a
//! connection has authenticated it is held to [`HANDSHAKE_FRAME`].

use adoc_collab::protocol::{decode, frame_length, FrameError, Message};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The largest frame read before a connection has authenticated: a Hello or an Authenticate.
pub const HANDSHAKE_FRAME: usize = 4 << 10;

/// Why a frame could not be read.
#[derive(Debug)]
pub enum ReadError {
    /// The stream ended or failed.
    Closed,
    /// The bytes are not a frame of the protocol, or the frame is over `limit`.
    Frame(FrameError),
}

/// Reads one frame of at most `limit` bytes after its length prefix.
pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> Result<Message, ReadError> {
    let mut prefix = [0; 4];
    reader
        .read_exact(&mut prefix)
        .await
        .map_err(|_| ReadError::Closed)?;
    let length = frame_length(prefix).map_err(ReadError::Frame)?;
    if length > limit {
        return Err(ReadError::Frame(FrameError::TooLarge(length)));
    }
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|_| ReadError::Closed)?;
    decode(&body).map_err(ReadError::Frame)
}

/// Writes an encoded frame and flushes it.
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &[u8],
) -> std::io::Result<()> {
    writer.write_all(frame).await?;
    writer.flush().await
}

/// Encodes and writes `message`.
pub async fn write_message<W: AsyncWrite + Unpin>(
    writer: &mut W,
    message: &Message,
) -> std::io::Result<()> {
    let frame = message.encode().map_err(|error| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string())
    })?;
    write_frame(writer, &frame).await
}
