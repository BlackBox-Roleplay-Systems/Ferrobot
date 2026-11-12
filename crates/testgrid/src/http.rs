//! A minimal HTTP/1.1 server for the login and the capabilities: one request per connection,
//! `Content-Length` bodies, `Connection: close` replies. Enough for the bot's client; no
//! dependency on a web framework.

use crate::Shared;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MAX_REQUEST: usize = 1 << 20;

pub struct Request {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
}

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Response {
            status,
            content_type,
            body: body.into(),
        }
    }

    pub fn status(status: u16) -> Self {
        Response::new(status, "text/plain", Vec::new())
    }
}

pub async fn serve(listener: TcpListener, shared: Arc<Shared>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(connection(stream, shared.clone()));
            }
            Err(e) => log::warn!("testgrid http accept: {e}"),
        }
    }
}

async fn connection(mut stream: TcpStream, shared: Arc<Shared>) {
    let req = match read_request(&mut stream).await {
        Ok(req) => req,
        Err(e) => {
            log::debug!("testgrid http: {e}");
            return;
        }
    };
    let resp = route(&shared, req).await;
    let reason = match resp.status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        502 => "Bad Gateway",
        _ => "Error",
    };
    let mut out = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        resp.status,
        resp.content_type,
        resp.body.len()
    )
    .into_bytes();
    out.extend_from_slice(&resp.body);
    if stream.write_all(&out).await.is_err() {
        return;
    }
    // Shut the write side, then wait for the client to close: dropping the socket at once can
    // reset the connection and lose the reply (Aurora's test_http.rs).
    let _ = stream.shutdown().await;
    let mut sink = [0u8; 1024];
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while matches!(stream.read(&mut sink).await, Ok(n) if n > 0) {}
    })
    .await;
}

async fn route(shared: &Arc<Shared>, req: Request) -> Response {
    let (path, query) = req.path.split_once('?').unwrap_or((&req.path, ""));
    if path == "/login.cgi" {
        if req.method != "POST" {
            return Response::status(405);
        }
        return crate::login::handle(shared, &req.body);
    }
    // /cap/<session>/<region>/<name>, GetDisplayNames with a slash and a query after it
    let mut parts = path.trim_matches('/').split('/');
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some("cap"), Some(n), Some(region), Some(name), None) => {
            match (n.parse(), region.parse()) {
                (Ok(n), Ok(region)) => {
                    crate::caps::handle(shared, n, region, name, &req.method, query, &req.body)
                        .await
                }
                _ => Response::status(404),
            }
        }
        _ => Response::status(404),
    }
}

async fn read_request(stream: &mut TcpStream) -> Result<Request, String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > MAX_REQUEST {
            return Err("request head too large".into());
        }
        let n = stream.read(&mut chunk).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("connection closed before the request".into());
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let method = first.next().unwrap_or("").to_owned();
    let path = first.next().unwrap_or("").to_owned();
    let mut length = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().map_err(|_| "bad Content-Length")?;
        }
    }
    if length > MAX_REQUEST {
        return Err("request body too large".into());
    }
    let mut body = buf[head_end..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("connection closed in the body".into());
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Ok(Request { method, path, body })
}
