//! Minimal blocking HTTP helpers used by cutex's local service endpoints.

use std::collections::HashMap;
use std::fmt;
use std::io::Read;
use std::io::Write;
use std::net::Shutdown;
use std::net::TcpStream;
use std::time::Duration;

use anyhow::Context;
use serde_json::Value;

#[derive(Debug)]
pub struct SimpleHttpRequest {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub struct HttpRequestBodyTooLarge {
    pub method: String,
    pub path: String,
    pub content_length: usize,
    pub limit: usize,
}

impl fmt::Display for HttpRequestBodyTooLarge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "HTTP request body is {} bytes, exceeding the {} byte limit",
            self.content_length, self.limit
        )
    }
}

impl std::error::Error for HttpRequestBodyTooLarge {}

pub fn read_simple_http_request(stream: &mut TcpStream) -> anyhow::Result<SimpleHttpRequest> {
    read_simple_http_request_with_body_limit(stream, 1024 * 1024)
}

pub fn read_simple_http_request_with_body_limit(
    stream: &mut TcpStream,
    body_limit: usize,
) -> anyhow::Result<SimpleHttpRequest> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut buf = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1024];
        let n = stream
            .read(&mut chunk)
            .context("Failed to read HTTP request")?;
        if n == 0 {
            anyhow::bail!("Connection closed before HTTP headers");
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 1024 * 1024 {
            anyhow::bail!("HTTP request headers are too large");
        }
        if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos + 4;
        }
    };

    let headers_text = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = headers_text.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| anyhow::anyhow!("Missing HTTP request line"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("Missing HTTP method"))?
        .to_string();
    let path = request_parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("Missing HTTP path"))?
        .to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let content_length = headers
        .get("content-length")
        .map(|value| {
            value
                .parse::<usize>()
                .context("Invalid HTTP Content-Length")
        })
        .transpose()?
        .unwrap_or(0);
    if content_length > body_limit {
        return Err(HttpRequestBodyTooLarge {
            method,
            path,
            content_length,
            limit: body_limit,
        }
        .into());
    }
    while buf.len() < header_end + content_length {
        let mut chunk = [0_u8; 1024];
        let n = stream
            .read(&mut chunk)
            .context("Failed to read HTTP body")?;
        if n == 0 {
            anyhow::bail!("Connection closed before HTTP body");
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = buf[header_end..header_end + content_length].to_vec();
    Ok(SimpleHttpRequest {
        method,
        path,
        headers,
        body,
    })
}

pub fn require_bridge_token(
    request: &SimpleHttpRequest,
    token: Option<&str>,
) -> anyhow::Result<()> {
    require_service_bridge_token(request, token, "desktop notify")
}

pub fn require_service_bridge_token(
    request: &SimpleHttpRequest,
    token: Option<&str>,
    service: &str,
) -> anyhow::Result<()> {
    let Some(token) = token.filter(|token| !token.is_empty()) else {
        return Ok(());
    };
    let expected = format!("Bearer {token}");
    let actual = request
        .headers
        .get("authorization")
        .map(String::as_str)
        .unwrap_or("");
    if actual != expected {
        anyhow::bail!("Unauthorized {service} request");
    }
    Ok(())
}

pub fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> anyhow::Result<()> {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .context("Failed to write HTTP response headers")?;
    stream
        .write_all(body)
        .context("Failed to write HTTP response body")?;
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

pub fn write_json_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    value: &Value,
) -> anyhow::Result<()> {
    let body = serde_json::to_vec(value)?;
    write_http_response(stream, status, reason, "application/json", &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_auth_failure_names_the_calling_service_without_leaking_the_token() {
        let request = SimpleHttpRequest {
            method: "GET".to_string(),
            path: "/api/agents".to_string(),
            headers: HashMap::new(),
            body: Vec::new(),
        };
        let error =
            require_service_bridge_token(&request, Some("secret-bearing-token"), "Agent Bus")
                .unwrap_err()
                .to_string();
        assert_eq!(error, "Unauthorized Agent Bus request");
        assert!(!error.contains("secret-bearing-token"));
        assert!(!error.contains("desktop notify"));
    }
}
