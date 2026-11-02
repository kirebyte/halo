//! Minimal HTTP/1.1 for the bridges that speak JSON (GameSense): POST with a body, keep-alive,
//! `Expect: 100-continue` and `Transfer-Encoding: chunked`. Only listens on 127.0.0.1.

use std::io;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

const MAX_HEADERS: usize = 16 * 1024;
const MAX_BODY: usize = 4 * 1024 * 1024;
const CORS: &str = "Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\n";

pub struct Request {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
    /// Browsers send it on cross-site requests; games do not.
    pub origin: Option<String>,
    close: bool,
}

pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    pub fn ok(body: impl Into<String>) -> Self {
        Self { status: 200, body: body.into() }
    }

    pub fn error(status: u16, msg: &str) -> Self {
        Self { status, body: serde_json::json!({ "error": msg }).to_string() }
    }
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Serves requests on the connection until the client closes it. With `cors`, any web page may
/// call (which is what Razer's REST server does).
pub async fn serve<F, Fut>(stream: TcpStream, cors: bool, mut handle: F) -> io::Result<()>
where
    F: FnMut(Request) -> Fut,
    Fut: Future<Output = Response>,
{
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    loop {
        let Some(req) = read_request(&mut reader, &mut write).await? else { return Ok(()) };
        let close = req.close;
        let resp = handle(req).await;
        let reason = match resp.status {
            200 => "OK",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            _ => "Error",
        };
        let head = format!(
            "HTTP/1.1 {} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{}{}\r\n",
            resp.status,
            resp.body.len(),
            if cors { CORS } else { "" },
            if close { "Connection: close\r\n" } else { "" }
        );
        write.write_all(head.as_bytes()).await?;
        write.write_all(resp.body.as_bytes()).await?;
        if close {
            return Ok(());
        }
    }
}

async fn read_line<R: AsyncBufReadExt + Unpin>(r: &mut R, budget: &mut usize) -> io::Result<Option<String>> {
    let mut line = String::new();
    let n = r.read_line(&mut line).await?;
    if n == 0 {
        return Ok(None);
    }
    *budget = budget.checked_sub(n).ok_or_else(|| bad("headers too large"))?;
    Ok(Some(line.trim_end_matches(['\r', '\n']).to_string()))
}

async fn read_request<R, W>(r: &mut R, w: &mut W) -> io::Result<Option<Request>>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut budget = MAX_HEADERS;
    let Some(first) = read_line(r, &mut budget).await? else { return Ok(None) };
    let mut parts = first.split_whitespace();
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(bad("invalid request line"));
    };
    let path = target.split('?').next().unwrap_or("").to_string();
    let mut len = 0usize;
    let mut chunked = false;
    let mut close = version == "HTTP/1.0";
    let mut origin = None;
    let mut expect = false;
    loop {
        let line = read_line(r, &mut budget).await?.ok_or_else(|| bad("request cut short"))?;
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => len = value.parse().map_err(|_| bad("invalid Content-Length"))?,
            "transfer-encoding" => chunked = value.to_ascii_lowercase().contains("chunked"),
            "connection" => close = value.eq_ignore_ascii_case("close"),
            "origin" => origin = Some(value.to_string()),
            "expect" => expect = value.eq_ignore_ascii_case("100-continue"),
            _ => {}
        }
    }
    if expect {
        w.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await?;
    }
    let body = if chunked {
        let mut body = Vec::new();
        loop {
            let size_line = read_line(r, &mut budget).await?.ok_or_else(|| bad("chunk cut short"))?;
            let size = usize::from_str_radix(size_line.split(';').next().unwrap_or("").trim(), 16).map_err(|_| bad("invalid chunk"))?;
            if size == 0 {
                while !read_line(r, &mut budget).await?.unwrap_or_default().is_empty() {}
                break;
            }
            if body.len() + size > MAX_BODY {
                return Err(bad("body too large"));
            }
            let start = body.len();
            body.resize(start + size, 0);
            r.read_exact(&mut body[start..]).await?;
            let mut crlf = [0u8; 2];
            r.read_exact(&mut crlf).await?;
        }
        body
    } else {
        if len > MAX_BODY {
            return Err(bad("body too large"));
        }
        let mut body = vec![0u8; len];
        r.read_exact(&mut body).await?;
        body
    };
    Ok(Some(Request { method: method.to_string(), path, body, origin, close }))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn parse(raw: &[u8]) -> (Option<Request>, Vec<u8>) {
        let mut r = BufReader::new(raw);
        let mut w = Vec::new();
        let req = read_request(&mut r, &mut w).await.unwrap();
        (req, w)
    }

    #[tokio::test]
    async fn content_length_and_expect() {
        let (req, w) = parse(b"POST /game_event HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\nExpect: 100-continue\r\n\r\n{}").await;
        let req = req.unwrap();
        assert_eq!((req.method.as_str(), req.path.as_str(), req.body.as_slice()), ("POST", "/game_event", b"{}".as_slice()));
        assert!(req.origin.is_none() && !req.close);
        assert!(w.starts_with(b"HTTP/1.1 100 Continue"));
    }

    #[tokio::test]
    async fn chunked_origin_and_close() {
        let raw = b"POST /a?x=1 HTTP/1.1\r\nTransfer-Encoding: chunked\r\nOrigin: https://web\r\nConnection: close\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        let req = parse(raw).await.0.unwrap();
        assert_eq!((req.path.as_str(), req.body.as_slice()), ("/a", b"abcde".as_slice()));
        assert_eq!(req.origin.as_deref(), Some("https://web"));
        assert!(req.close);
    }

    #[tokio::test]
    async fn eof_is_none() {
        assert!(parse(b"").await.0.is_none());
    }
}
