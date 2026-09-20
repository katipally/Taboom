use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtoError {
    #[error("frame too large: {size} bytes (max {max})")]
    FrameTooLarge { size: usize, max: usize },

    #[error("unexpected end of stream")]
    UnexpectedEof,

    #[error("CBOR decode error: {0}")]
    CborDecode(String),

    #[error("CBOR encode error: {0}")]
    CborEncode(String),

    #[error("protocol version mismatch: got {got}, expected {expected}")]
    VersionMismatch { got: u32, expected: u32 },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("unknown message type: {0}")]
    UnknownMessageType(u8),
}
