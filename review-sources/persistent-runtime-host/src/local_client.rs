#[cfg(any(unix, target_os = "windows"))]
use crate::protocol::EventEnvelope;
use crate::protocol::{RequestEnvelope, ResponseEnvelope};
#[cfg(any(unix, target_os = "windows"))]
use crate::transport::{read_json_frame, write_json_frame};
use std::fmt;
#[cfg(any(unix, target_os = "windows"))]
use std::io::BufReader;
use std::path::{Path, PathBuf};

#[cfg(unix)]
type PlatformStream = std::os::unix::net::UnixStream;
#[cfg(target_os = "windows")]
type PlatformStream = crate::windows_pipe::WindowsPipeStream;

pub struct LocalClient {
    socket_path: PathBuf,
}

impl LocalClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    #[cfg(any(unix, target_os = "windows"))]
    pub fn call(&self, request: &RequestEnvelope) -> Result<ResponseEnvelope, LocalClientError> {
        let mut stream = connect_platform(&self.socket_path).map_err(|error| {
            LocalClientError::HostUnavailable(format!(
                "cannot connect to {}: {error}; hostctl never starts PRH implicitly",
                self.socket_path.display()
            ))
        })?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .map_err(LocalClientError::Transport)?;
        stream
            .set_write_timeout(Some(std::time::Duration::from_secs(30)))
            .map_err(LocalClientError::Transport)?;
        write_json_frame(&mut stream, request).map_err(LocalClientError::Transport)?;
        let mut reader = BufReader::new(stream);
        read_json_frame(&mut reader)
            .map_err(LocalClientError::Transport)?
            .ok_or_else(|| {
                LocalClientError::Transport(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "host closed before sending a response",
                ))
            })
    }

    #[cfg(not(any(unix, target_os = "windows")))]
    pub fn call(&self, _request: &RequestEnvelope) -> Result<ResponseEnvelope, LocalClientError> {
        Err(LocalClientError::HostUnavailable(
            "the PRH local client is available only on Linux and Windows".to_owned(),
        ))
    }

    #[cfg(any(unix, target_os = "windows"))]
    pub fn subscribe(
        &self,
        request: &RequestEnvelope,
    ) -> Result<(ResponseEnvelope, LocalEventReader), LocalClientError> {
        let mut stream = connect_platform(&self.socket_path).map_err(|error| {
            LocalClientError::HostUnavailable(format!(
                "cannot connect to {}: {error}; hostctl never starts PRH implicitly",
                self.socket_path.display()
            ))
        })?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .map_err(LocalClientError::Transport)?;
        stream
            .set_write_timeout(Some(std::time::Duration::from_secs(30)))
            .map_err(LocalClientError::Transport)?;
        write_json_frame(&mut stream, request).map_err(LocalClientError::Transport)?;
        let mut reader = BufReader::new(stream);
        let response = read_json_frame(&mut reader)
            .map_err(LocalClientError::Transport)?
            .ok_or_else(|| {
                LocalClientError::Transport(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "host closed before accepting the subscription",
                ))
            })?;
        reader
            .get_mut()
            .set_read_timeout(None)
            .map_err(LocalClientError::Transport)?;
        Ok((response, LocalEventReader { reader }))
    }
}

#[cfg(any(unix, target_os = "windows"))]
pub struct LocalEventReader {
    reader: BufReader<PlatformStream>,
}

#[cfg(any(unix, target_os = "windows"))]
impl LocalEventReader {
    pub fn set_read_timeout(
        &self,
        timeout: Option<std::time::Duration>,
    ) -> Result<(), LocalClientError> {
        self.reader
            .get_ref()
            .set_read_timeout(timeout)
            .map_err(LocalClientError::Transport)
    }

    pub fn next_event(&mut self) -> Result<Option<EventEnvelope>, LocalClientError> {
        read_json_frame(&mut self.reader).map_err(LocalClientError::Transport)
    }
}

#[cfg(unix)]
fn connect_platform(path: &Path) -> std::io::Result<PlatformStream> {
    std::os::unix::net::UnixStream::connect(path)
}

#[cfg(target_os = "windows")]
fn connect_platform(path: &Path) -> std::io::Result<PlatformStream> {
    crate::windows_pipe::WindowsPipeStream::connect(path)
}

#[derive(Debug)]
pub enum LocalClientError {
    HostUnavailable(String),
    Transport(std::io::Error),
}

impl fmt::Display for LocalClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HostUnavailable(message) => message.fmt(f),
            Self::Transport(error) => write!(f, "local API transport failed: {error}"),
        }
    }
}

impl std::error::Error for LocalClientError {}
