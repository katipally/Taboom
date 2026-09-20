use crate::error::ProtoError;
use crate::message::Envelope;
use bytes::{Buf, BufMut, BytesMut};
use std::io::Cursor;

pub const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;
const LENGTH_PREFIX_SIZE: usize = 4;

pub struct FrameWriter;
pub struct FrameReader;

impl FrameWriter {
    pub fn encode(envelope: &Envelope) -> Result<Vec<u8>, ProtoError> {
        let mut cbor_buf = Vec::new();
        ciborium::into_writer(envelope, &mut cbor_buf)
            .map_err(|e| ProtoError::CborEncode(e.to_string()))?;

        if cbor_buf.len() > MAX_FRAME_SIZE {
            return Err(ProtoError::FrameTooLarge {
                size: cbor_buf.len(),
                max: MAX_FRAME_SIZE,
            });
        }

        let mut out = Vec::with_capacity(LENGTH_PREFIX_SIZE + cbor_buf.len());
        out.put_u32(cbor_buf.len() as u32);
        out.extend_from_slice(&cbor_buf);
        Ok(out)
    }
}

impl FrameReader {
    pub fn try_decode(buf: &mut BytesMut) -> Result<Option<Envelope>, ProtoError> {
        if buf.len() < LENGTH_PREFIX_SIZE {
            return Ok(None);
        }

        let mut peek = Cursor::new(&buf[..]);
        let frame_len = peek.get_u32() as usize;

        if frame_len > MAX_FRAME_SIZE {
            return Err(ProtoError::FrameTooLarge {
                size: frame_len,
                max: MAX_FRAME_SIZE,
            });
        }

        if buf.len() < LENGTH_PREFIX_SIZE + frame_len {
            return Ok(None);
        }

        buf.advance(LENGTH_PREFIX_SIZE);
        let frame_bytes = buf.split_to(frame_len);

        let envelope: Envelope = ciborium::from_reader(&frame_bytes[..])
            .map_err(|e| ProtoError::CborDecode(e.to_string()))?;

        Ok(Some(envelope))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Heartbeat, Message};

    #[test]
    fn roundtrip() {
        let env = Envelope::new(Message::Heartbeat(Heartbeat { uptime_secs: 42 }));
        let encoded = FrameWriter::encode(&env).unwrap();
        let mut buf = BytesMut::from(&encoded[..]);
        let decoded = FrameReader::try_decode(&mut buf).unwrap().unwrap();
        assert_eq!(env.id, decoded.id);
    }
}
