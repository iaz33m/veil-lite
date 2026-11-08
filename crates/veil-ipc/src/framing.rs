//! Length-prefixed JSON frames: 4-byte little-endian length, then UTF-8 JSON.

use crate::protocol::MAX_FRAME;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::{self, Read, Write};

/// Serialize `value` and write it as one frame.
pub fn write_frame<W: Write, T: Serialize>(w: &mut W, value: &T) -> io::Result<()> {
    let body = serde_json::to_vec(value).map_err(io::Error::other)?;
    if body.len() > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "frame too large"));
    }
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(&body);
    w.write_all(&buf)?;
    w.flush()
}

/// Read one frame and deserialize it. Frames over [`MAX_FRAME`] are rejected without reading the body.
pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("frame of {len} bytes exceeds the limit")));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Request;

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &Request::Activity { limit: 5 }).unwrap();
        write_frame(&mut buf, &Request::Status).unwrap();
        let mut r = io::Cursor::new(buf);
        assert_eq!(read_frame::<_, Request>(&mut r).unwrap(), Request::Activity { limit: 5 });
        assert_eq!(read_frame::<_, Request>(&mut r).unwrap(), Request::Status);
    }

    #[test]
    fn oversize_frames_are_rejected() {
        let mut buf = ((MAX_FRAME + 1) as u32).to_le_bytes().to_vec();
        buf.extend_from_slice(b"{}");
        let err = read_frame::<_, Request>(&mut io::Cursor::new(buf)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let huge = Request::SetPolicy { yaml: "x".repeat(MAX_FRAME) };
        assert!(write_frame(&mut Vec::new(), &huge).is_err());
    }

    #[test]
    fn garbage_is_invalid_data() {
        let mut buf = 3u32.to_le_bytes().to_vec();
        buf.extend_from_slice(b"{x}");
        assert_eq!(read_frame::<_, Request>(&mut io::Cursor::new(buf)).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
