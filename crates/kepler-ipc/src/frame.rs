//! Newline-delimited frame reader/writer with a hard size limit.

use std::io;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug)]
pub enum FrameError {
    /// Frame exceeded `max` bytes before a newline was seen.
    TooLarge,
    /// Stream ended in the middle of a frame.
    Truncated,
    Io(io::Error),
}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => f.write_str("frame too large"),
            Self::Truncated => f.write_str("connection closed mid-frame"),
            Self::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Read one frame. `Ok(None)` is a clean EOF at a frame boundary. Memory use is
/// bounded by `max` plus the reader's buffer, never by what the peer sends.
pub async fn read_frame<R: AsyncBufRead + Unpin>(
    r: &mut R,
    max: usize,
) -> Result<Option<Vec<u8>>, FrameError> {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let (done, consumed) = {
            let chunk = r.fill_buf().await?;
            if chunk.is_empty() {
                return if buf.is_empty() {
                    Ok(None)
                } else {
                    Err(FrameError::Truncated)
                };
            }
            match chunk.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    if buf.len() + i > max {
                        return Err(FrameError::TooLarge);
                    }
                    buf.extend_from_slice(&chunk[..i]);
                    (true, i + 1)
                }
                None => {
                    if buf.len() + chunk.len() > max {
                        return Err(FrameError::TooLarge);
                    }
                    buf.extend_from_slice(chunk);
                    (false, chunk.len())
                }
            }
        };
        r.consume(consumed);
        if done {
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            return Ok(Some(buf));
        }
    }
}

/// Serialize `value` as one frame (JSON + `\n`) and flush.
pub async fn write_json<W: AsyncWrite + Unpin, T: serde::Serialize>(
    w: &mut W,
    value: &T,
) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    w.write_all(&bytes).await?;
    w.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    async fn read_all(input: &[u8], max: usize) -> Vec<Result<Option<Vec<u8>>, String>> {
        let mut r = BufReader::with_capacity(8, input);
        let mut out = Vec::new();
        loop {
            match read_frame(&mut r, max).await {
                Ok(None) => {
                    out.push(Ok(None));
                    return out;
                }
                Ok(Some(f)) => out.push(Ok(Some(f))),
                Err(e) => {
                    out.push(Err(e.to_string()));
                    return out;
                }
            }
        }
    }

    #[tokio::test]
    async fn splits_frames_across_small_buffers() {
        let out = read_all(b"{\"a\":1}\n{\"b\":2}\r\n\n", 64).await;
        assert_eq!(out[0].as_ref().unwrap().as_deref(), Some(&b"{\"a\":1}"[..]));
        assert_eq!(out[1].as_ref().unwrap().as_deref(), Some(&b"{\"b\":2}"[..]));
        assert_eq!(out[2].as_ref().unwrap().as_deref(), Some(&b""[..]));
        assert!(matches!(out[3], Ok(None)));
    }

    #[tokio::test]
    async fn rejects_oversized_frames_with_and_without_newline() {
        let out = read_all(b"0123456789\n", 9).await;
        assert!(out[0].is_err());
        let out = read_all(b"0123456789", 9).await;
        assert!(out[0].is_err());
        // exactly max is fine
        let out = read_all(b"012345678\n", 9).await;
        assert!(out[0].is_ok());
    }

    #[tokio::test]
    async fn truncated_frame_is_an_error() {
        let out = read_all(b"abc", 64).await;
        assert!(out[0].is_err());
    }

    #[tokio::test]
    async fn unbounded_input_never_buffers_more_than_max() {
        // 10 MB without a newline must fail quickly after max bytes.
        let data = vec![b'x'; 10 * 1024 * 1024];
        let mut r = BufReader::new(&data[..]);
        assert!(matches!(
            read_frame(&mut r, 1024).await,
            Err(FrameError::TooLarge)
        ));
    }
}
