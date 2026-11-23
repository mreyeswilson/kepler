//! A deliberately tiny HTTP/1.1 subset: one request per connection, no keep-alive, no chunked
//! bodies, bounded everything. Enough for a loopback UI, small enough to audit.

use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const MAX_HEADER_BYTES: usize = 16 * 1024;
pub const MAX_BODY_BYTES: usize = 4 * 1024;
pub const MAX_HEADERS: usize = 64;
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    /// Header names lowercased. Duplicate headers keep the first value.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    TooLarge,
    Malformed,
    Timeout,
    Closed,
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_head(head: &[u8]) -> Result<Request, ParseError> {
    let text = std::str::from_utf8(head).map_err(|_| ParseError::Malformed)?;
    let mut lines = text.split("\r\n");
    let first = lines.next().ok_or(ParseError::Malformed)?;
    let mut parts = first.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ParseError::Malformed);
    };
    if !version.starts_with("HTTP/1.") || method.is_empty() || !target.starts_with('/') {
        return Err(ParseError::Malformed);
    }
    let (path, qs) = target.split_once('?').unwrap_or((target, ""));
    let mut query = HashMap::new();
    for pair in qs.split('&').filter(|p| !p.is_empty()).take(16) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        query
            .entry(percent_decode(k))
            .or_insert_with(|| percent_decode(v));
    }
    let mut headers = HashMap::new();
    for (n, line) in lines.enumerate() {
        if line.is_empty() {
            break;
        }
        if n >= MAX_HEADERS {
            return Err(ParseError::TooLarge);
        }
        let (k, v) = line.split_once(':').ok_or(ParseError::Malformed)?;
        if k.is_empty() || k.contains(char::is_whitespace) {
            return Err(ParseError::Malformed);
        }
        headers
            .entry(k.to_ascii_lowercase())
            .or_insert_with(|| v.trim().to_string());
    }
    Ok(Request {
        method: method.to_string(),
        path: percent_decode(path),
        query,
        headers,
        body: Vec::new(),
    })
}

/// Read one request (head and, for a declared Content-Length, a small body).
pub async fn read_request(stream: &mut TcpStream) -> Result<Request, ParseError> {
    let work = async {
        let mut buf: Vec<u8> = Vec::with_capacity(1024);
        let mut chunk = [0u8; 2048];
        let head_end = loop {
            if let Some(i) = find(&buf, b"\r\n\r\n") {
                break i;
            }
            if buf.len() > MAX_HEADER_BYTES {
                return Err(ParseError::TooLarge);
            }
            let n = stream
                .read(&mut chunk)
                .await
                .map_err(|_| ParseError::Closed)?;
            if n == 0 {
                return Err(ParseError::Closed);
            }
            buf.extend_from_slice(&chunk[..n]);
        };
        if head_end > MAX_HEADER_BYTES {
            return Err(ParseError::TooLarge);
        }
        let mut req = parse_head(&buf[..head_end])?;
        if req.headers.contains_key("transfer-encoding") {
            return Err(ParseError::Malformed);
        }
        let len: usize = match req.headers.get("content-length") {
            Some(v) => v.parse().map_err(|_| ParseError::Malformed)?,
            None => 0,
        };
        if len > MAX_BODY_BYTES {
            return Err(ParseError::TooLarge);
        }
        let mut body = buf[head_end + 4..].to_vec();
        while body.len() < len {
            let n = stream
                .read(&mut chunk)
                .await
                .map_err(|_| ParseError::Closed)?;
            if n == 0 {
                return Err(ParseError::Closed);
            }
            body.extend_from_slice(&chunk[..n]);
            if body.len() > MAX_BODY_BYTES + 2048 {
                return Err(ParseError::TooLarge);
            }
        }
        body.truncate(len);
        req.body = body;
        Ok(req)
    };
    match tokio::time::timeout(READ_TIMEOUT, work).await {
        Ok(r) => r,
        Err(_) => Err(ParseError::Timeout),
    }
}

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub extra: Vec<(&'static str, String)>,
}

impl Response {
    pub fn new(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type,
            body: body.into(),
            extra: Vec::new(),
        }
    }

    pub fn json(status: u16, v: &serde_json::Value) -> Self {
        Self::new(status, "application/json; charset=utf-8", v.to_string())
    }

    pub fn text(status: u16, msg: &str) -> Self {
        Self::new(status, "text/plain; charset=utf-8", msg.to_string())
    }

    pub fn header(mut self, k: &'static str, v: String) -> Self {
        // Header injection guard: values are built by this crate, but never trust that.
        let v: String = v.chars().filter(|c| !c.is_control()).collect();
        self.extra.push((k, v));
        self
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        421 => "Misdirected Request",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

pub async fn write_response(stream: &mut TcpStream, r: &Response, head_only: bool) {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\
         Cross-Origin-Resource-Policy: same-origin\r\nCross-Origin-Opener-Policy: same-origin\r\n\
         X-Frame-Options: DENY\r\n\
         Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; \
         connect-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'none'; \
         frame-ancestors 'none'\r\n",
        r.status,
        reason(r.status),
        r.content_type,
        r.body.len()
    );
    for (k, v) in &r.extra {
        head.push_str(k);
        head.push_str(": ");
        head.push_str(v);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes()).await;
    if !head_only {
        let _ = stream.write_all(&r.body).await;
    }
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_target_query_and_headers() {
        let r = parse_head(
            b"GET /api/x%20y?t=ab%2Fc&n=1 HTTP/1.1\r\nHost: 127.0.0.1:80\r\nX-A: 1\r\nx-a: 2",
        )
        .unwrap();
        assert_eq!(r.path, "/api/x y");
        assert_eq!(r.query["t"], "ab/c");
        assert_eq!(r.headers["host"], "127.0.0.1:80");
        assert_eq!(r.headers["x-a"], "1", "first duplicate wins");
    }

    #[test]
    fn rejects_malformed_heads() {
        for bad in [
            &b"GET / HTTP/1.1 extra\r\n"[..],
            b"GET noslash HTTP/1.1\r\n",
            b"GET / SPDY/3\r\n",
            b"GET / HTTP/1.1\r\nbad header line\r\n",
            b"GET / HTTP/1.1\r\n x: folded\r\n",
            b"\xff\xfe / HTTP/1.1\r\n",
        ] {
            assert_eq!(
                parse_head(bad).err(),
                Some(ParseError::Malformed),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn percent_decoding_is_total() {
        assert_eq!(percent_decode("%"), "%");
        assert_eq!(percent_decode("%4"), "%4");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("a%41"), "aA");
    }
}
