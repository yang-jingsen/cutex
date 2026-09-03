//! Minimal blocking HTTP client helpers for local cutex services.

use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpStream;
use std::time::Duration;

use anyhow::Context;
use serde_json::Value;
use url::Url;

pub struct HttpJsonRequest<'a> {
    pub url: &'a str,
    pub method: &'a str,
    pub token: Option<&'a str>,
    pub body: Option<&'a [u8]>,
    pub timeout: Duration,
    pub invalid_url_context: &'a str,
    pub only_http_message: &'a str,
    pub missing_host_message: &'a str,
    pub connect_context: &'a str,
    pub read_context: &'a str,
    pub non_success_prefix: &'a str,
    pub parse_context: &'a str,
    pub ok_text_as_null: bool,
}

pub struct HttpPostStatusRequest<'a> {
    pub url: &'a str,
    pub token: Option<&'a str>,
    pub body: &'a [u8],
    pub timeout: Duration,
    pub invalid_url_context: &'a str,
    pub only_http_message: &'a str,
    pub missing_host_message: &'a str,
    pub connect_context: &'a str,
    pub non_success_message: &'a str,
}

pub fn http_json_request(options: HttpJsonRequest<'_>) -> anyhow::Result<Value> {
    let url = Url::parse(options.url).with_context(|| options.invalid_url_context.to_string())?;
    if url.scheme() != "http" {
        anyhow::bail!("{}", options.only_http_message);
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("{}", options.missing_host_message))?;
    let port = url.port_or_known_default().unwrap_or(80);
    let addr = format!("{host}:{port}");
    let mut stream =
        TcpStream::connect(addr).with_context(|| options.connect_context.to_string())?;
    stream.set_write_timeout(Some(options.timeout)).ok();
    stream.set_read_timeout(Some(options.timeout)).ok();
    let mut request_path = url.path().to_string();
    if request_path.is_empty() {
        request_path.push('/');
    }
    if let Some(query) = url.query() {
        request_path.push('?');
        request_path.push_str(query);
    }
    let body = options.body.unwrap_or(b"");
    let auth = options
        .token
        .filter(|token| !token.is_empty())
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let content_type = if body.is_empty() {
        String::new()
    } else {
        "Content-Type: application/json\r\n".to_string()
    };
    let request = format!(
        "{} {request_path} HTTP/1.1\r\nHost: {host}:{port}\r\n{auth}{content_type}Content-Length: {}\r\nConnection: close\r\n\r\n",
        options.method,
        body.len()
    );
    stream.write_all(request.as_bytes())?;
    stream.write_all(body)?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .with_context(|| options.read_context.to_string())?;
    let text = String::from_utf8_lossy(&response);
    let (headers, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_ref(), ""));
    if !headers.starts_with("HTTP/1.1 2") {
        anyhow::bail!("{}: {headers}\n{body}", options.non_success_prefix);
    }
    let body = body.trim();
    if body.is_empty() || (options.ok_text_as_null && body == "ok") {
        return Ok(Value::Null);
    }
    serde_json::from_str(body).with_context(|| options.parse_context.to_string())
}

pub fn http_post_json_expect_success(options: HttpPostStatusRequest<'_>) -> anyhow::Result<()> {
    let url = Url::parse(options.url).with_context(|| options.invalid_url_context.to_string())?;
    if url.scheme() != "http" {
        anyhow::bail!("{}", options.only_http_message);
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("{}", options.missing_host_message))?;
    let port = url.port_or_known_default().unwrap_or(80);
    let addr = format!("{host}:{port}");
    let mut stream =
        TcpStream::connect(addr).with_context(|| options.connect_context.to_string())?;
    stream.set_write_timeout(Some(options.timeout)).ok();
    stream.set_read_timeout(Some(options.timeout)).ok();
    let mut path = url.path().to_string();
    if path.is_empty() {
        path.push('/');
    }
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }
    let auth = options
        .token
        .filter(|token| !token.is_empty())
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n",
        options.body.len()
    );
    stream.write_all(request.as_bytes())?;
    stream.write_all(options.body)?;
    let mut response = [0_u8; 64];
    let n = stream.read(&mut response).unwrap_or(0);
    if n > 0 && !String::from_utf8_lossy(&response[..n]).starts_with("HTTP/1.1 2") {
        anyhow::bail!("{}", options.non_success_message);
    }
    Ok(())
}

pub fn http_local_root_status_ok(port: u16, token: Option<&str>, timeout: Duration) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, timeout) else {
        return false;
    };
    http_root_status_ok_on_stream(
        &mut stream,
        &format!("127.0.0.1:{port}"),
        token,
        timeout,
        false,
    )
}

pub fn http_base_url_root_status_ok(base_url: &str, timeout: Duration) -> bool {
    let Ok(url) = Url::parse(base_url) else {
        return false;
    };
    if url.scheme() != "http" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let port = url.port_or_known_default().unwrap_or(80);
    let Ok(mut stream) = TcpStream::connect(format!("{host}:{port}")) else {
        return false;
    };
    http_root_status_ok_on_stream(&mut stream, &format!("{host}:{port}"), None, timeout, true)
}

pub fn connect_local_port(port: u16, timeout: Duration) -> std::io::Result<TcpStream> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, timeout)
}

fn http_root_status_ok_on_stream(
    stream: &mut TcpStream,
    host_header: &str,
    token: Option<&str>,
    timeout: Duration,
    connection_close: bool,
) -> bool {
    stream.set_write_timeout(Some(timeout)).ok();
    stream.set_read_timeout(Some(timeout)).ok();
    let auth = token
        .filter(|token| !token.is_empty())
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let connection = if connection_close {
        "Connection: close\r\n"
    } else {
        ""
    };
    let request = format!(
        "GET / HTTP/1.1\r\nHost: {host_header}\r\n{auth}Content-Length: 0\r\n{connection}\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0_u8; 128];
    match stream.read(&mut buf) {
        Ok(n) => String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"),
        Err(_) => false,
    }
}
