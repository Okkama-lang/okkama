//! Frames: a 4-byte big-endian length, then that many bytes, the first of which says what the frame is.

use std::io::{self, Read, Write};

/// A message as UTF-8 JSON (`J`).
pub const FRAME_JSON: u8 = b'J';
/// A message as CBOR (`C`).
pub const FRAME_CBOR: u8 = b'C';
/// Raw bytes attached to the message before it (`B`).
pub const FRAME_ATTACHMENT: u8 = b'B';

/// The next frame's body (at least its type byte), or `None` when the input ends between frames. An input that
/// ends inside a frame, an empty frame and one over `limit` are protocol errors.
pub fn read(input: &mut impl Read, limit: usize) -> Result<Option<Vec<u8>>, crate::Error> {
    let mut length = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match input.read(&mut length[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(crate::Error::Malformed("the input ended inside a frame's length".into())),
            Ok(n) => got += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    let size = u32::from_be_bytes(length) as usize;
    if size == 0 {
        return Err(crate::Error::Malformed("an empty frame".into()));
    }
    if size > limit {
        return Err(crate::Error::Malformed(format!("a frame of {size} bytes, over the limit of {limit}")));
    }
    let mut body = vec![0u8; size];
    input.read_exact(&mut body).map_err(|error| match error.kind() {
        io::ErrorKind::UnexpectedEof => crate::Error::Malformed(format!("the input ended inside a frame of {size} bytes")),
        _ => error.into(),
    })?;
    Ok(Some(body))
}

/// Writes one frame; a body longer than 4 GiB has no length.
pub fn write(output: &mut impl Write, body: &[u8]) -> io::Result<()> {
    let size = u32::try_from(body.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a frame over 4 GiB"))?;
    output.write_all(&size.to_be_bytes())?;
    output.write_all(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_the_end_is_none() {
        let mut out = Vec::new();
        write(&mut out, b"Jhi").unwrap();
        write(&mut out, b"B").unwrap();
        assert_eq!(out, b"\0\0\0\x03Jhi\0\0\0\x01B");
        let mut input = &out[..];
        assert_eq!(read(&mut input, 16).unwrap().unwrap(), b"Jhi");
        assert_eq!(read(&mut input, 16).unwrap().unwrap(), b"B");
        assert!(read(&mut input, 16).unwrap().is_none());
    }

    #[test]
    fn broken_frames_are_protocol_errors() {
        let malformed = |bytes: &[u8], limit| match read(&mut &bytes[..], limit) {
            Err(crate::Error::Malformed(reason)) => reason,
            other => panic!("{other:?}"),
        };
        assert!(malformed(b"\0\0", 16).contains("inside a frame's length"));
        assert!(malformed(b"\0\0\0\0", 16).contains("empty"));
        assert!(malformed(b"\0\0\0\x11", 16).contains("over the limit"));
        assert!(malformed(b"\0\0\0\x05Jab", 16).contains("inside a frame of 5"));
    }
}
