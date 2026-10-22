//! Incremental length prefixed frame codec.

pub const MAGIC: [u8; 4] = *b"BRD1";
pub const HEADER_LEN: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub kind: u8,
    pub flags: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub header: Header,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodecError {
    BadMagic,
    BadReserved(u16),
    TooLarge { len: u32, max: usize },
    Truncated,
}

pub struct Encoder {
    _max_payload: usize,
}

impl Encoder {
    pub fn new(max_payload: usize) -> Self {
        Self {
            _max_payload: max_payload,
        }
    }

    pub fn encode(&self, _frame: &Frame) -> Result<Vec<u8>, CodecError> {
        todo!()
    }
}

pub struct Decoder {
    _max_payload: usize,
}

impl Decoder {
    pub fn new(max_payload: usize) -> Self {
        Self {
            _max_payload: max_payload,
        }
    }

    pub fn feed(&mut self, _bytes: &[u8]) -> Result<Vec<Frame>, CodecError> {
        todo!()
    }

    pub fn buffered(&self) -> usize {
        todo!()
    }

    pub fn finish(&self) -> Result<(), CodecError> {
        todo!()
    }
}
