use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::{self, BufRead, Write};

pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

pub fn read_json_frame<R: BufRead, T: DeserializeOwned>(reader: &mut R) -> io::Result<Option<T>> {
    let mut frame = Vec::new();
    // Allow a maximum-size JSON payload plus CRLF and one extra byte used to
    // prove an oversized frame without allocating beyond a fixed bound.
    let mut limited = std::io::Read::take(reader, (MAX_FRAME_BYTES + 3) as u64);
    let bytes_read = limited.read_until(b'\n', &mut frame)?;
    if bytes_read == 0 {
        return Ok(None);
    }
    if frame.last() == Some(&b'\n') {
        frame.pop();
        if frame.last() == Some(&b'\r') {
            frame.pop();
        }
    }
    if frame.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("local API frame exceeds {MAX_FRAME_BYTES} bytes"),
        ));
    }
    serde_json::from_slice(&frame)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn write_json_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> io::Result<()> {
    let encoded = serde_json::to_vec(value).map_err(io::Error::other)?;
    if encoded.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("local API frame exceeds {MAX_FRAME_BYTES} bytes"),
        ));
    }
    writer.write_all(&encoded)?;
    writer.write_all(b"\n")?;
    writer.flush()
}
