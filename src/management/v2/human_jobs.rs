//! Root-authenticated, bounded Job Service metadata bridge.
use crate::http::server::{write_json_response, SimpleHttpRequest};
use serde_json::{json, Value};
use std::net::TcpStream;

pub(super) fn handle(stream: &mut TcpStream, request: &SimpleHttpRequest) -> anyhow::Result<()> {
    let params: Value = match serde_json::from_slice(&request.body) {
        Ok(value) => value,
        Err(_) => {
            return super::server::write_v2_error(
                stream,
                400,
                "Bad Request",
                "invalid_request",
                "Invalid Job query",
                false,
                json!({}),
            )
        }
    };
    let valid = params.as_object().is_some_and(|object| {
        object.iter().all(|(k, v)| match k.as_str() {
            "query" => v.as_str().is_some_and(|s| s.len() <= 256),
            "cursor" => {
                v.is_null()
                    || v.as_str().is_some_and(|s| {
                        s.len() <= 160
                            && s.split_once(':').is_some_and(|(time, id)| {
                                time.parse::<u64>().is_ok() && !id.is_empty()
                            })
                    })
            }
            "limit" => v.is_null() || v.as_u64().is_some(),
            _ => false,
        })
    });
    if !valid {
        return super::server::write_v2_error(
            stream,
            400,
            "Bad Request",
            "invalid_request",
            "Expected query, cursor and limit for Jobs",
            false,
            json!({}),
        );
    }
    match query(params) {
        Ok(value) => write_json_response(stream, 200, "OK", &value),
        Err(error) => super::server::write_v2_error(
            stream,
            503,
            "Service Unavailable",
            "job_service_unavailable",
            &format!("{error:#}"),
            true,
            json!({}),
        ),
    }
}

#[cfg(not(target_os = "linux"))]
fn query(_: Value) -> anyhow::Result<Value> {
    anyhow::bail!("No local Job Service runner is available on this platform")
}

#[cfg(target_os = "linux")]
fn query(params: Value) -> anyhow::Result<Value> {
    use anyhow::Context;
    use std::io::{Read, Write};
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;
    let root = crate::config::paths::runtime_dir()?.join("job-service/v1");
    let path = root.join("job-service.sock");
    let bytes = path.as_os_str().as_bytes();
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    anyhow::ensure!(
        bytes.len() < addr.sun_path.len(),
        "Job socket path is too long"
    );
    addr.sun_family = libc::AF_UNIX as _;
    for (a, b) in addr.sun_path.iter_mut().zip(bytes) {
        *a = *b as _;
    }
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut socket = unsafe { UnixStream::from_raw_fd(fd) };
    let rc = unsafe {
        libc::connect(
            fd,
            &addr as *const _ as *const libc::sockaddr,
            std::mem::size_of_val(&addr) as _,
        )
    };
    if rc < 0 {
        let err = std::io::Error::last_os_error();
        anyhow::ensure!(
            err.raw_os_error() == Some(libc::EINPROGRESS),
            "Cannot connect to Job Service: {err}"
        );
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        anyhow::ensure!(
            unsafe { libc::poll(&mut poll, 1, 2000) } > 0,
            "Job Service connection timed out"
        );
        if let Some(err) = socket.take_error()? {
            return Err(err.into());
        }
    }
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    let token =
        std::fs::read(root.join("api.token")).context("Job Service credential unavailable")?;
    anyhow::ensure!(
        (32..=4096).contains(&token.len()),
        "Invalid Job Service credential size"
    );
    let token = token.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let mut body =
        serde_json::to_vec(&json!({"token":token,"method":"humanList","params":params}))?;
    anyhow::ensure!(body.len() < 16384, "Job request too large");
    body.push(b'\n');
    socket.write_all(&body)?;
    let mut reply = Vec::new();
    socket.take(1048577).read_to_end(&mut reply)?;
    anyhow::ensure!(reply.len() <= 1048576, "Job response exceeds 1 MiB");
    let mut reply: Value = serde_json::from_slice(&reply)?;
    anyhow::ensure!(
        reply["ok"] == true,
        "{}",
        reply["message"].as_str().unwrap_or("Job query failed")
    );
    let mut result = reply["result"].take();
    // Names are presentation only; ownership remains the service's session ID.
    if let Ok(store) = crate::session::store::load_cutex_session_store() {
        if let Some(rows) = result["data"].as_array_mut() {
            for row in rows {
                if let Some(id) = row["sessionId"].as_str() {
                    if let Some(record) = store.sessions.values().find(|r| r.cutex_session_id == id)
                    {
                        row["agentName"] =
                            json!(crate::session::metadata::cutex_session_display_name(record));
                    }
                }
            }
        }
    }
    Ok(result)
}
