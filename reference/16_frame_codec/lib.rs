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
    max_payload: usize,
}

impl Encoder {
    pub fn new(max_payload: usize) -> Self {
        Self { max_payload }
    }

    pub fn encode(&self, frame: &Frame) -> Result<Vec<u8>, CodecError> {
        let len = frame.payload.len();
        if len > u32::MAX as usize || len > self.max_payload {
            return Err(CodecError::TooLarge {
                len: len.min(u32::MAX as usize) as u32,
                max: self.max_payload,
            });
        }
        let mut out = Vec::with_capacity(HEADER_LEN + len);
        out.extend_from_slice(&MAGIC);
        out.push(frame.header.kind);
        out.push(frame.header.flags);
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&(len as u32).to_be_bytes());
        out.extend_from_slice(&frame.payload);
        Ok(out)
    }
}

pub struct Decoder {
    max_payload: usize,
    buf: Vec<u8>,
    poisoned: Option<CodecError>,
}

impl Decoder {
    pub fn new(max_payload: usize) -> Self {
        Self {
            max_payload,
            buf: Vec::new(),
            poisoned: None,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, CodecError> {
        if let Some(e) = &self.poisoned {
            return Err(e.clone());
        }
        self.buf.extend_from_slice(bytes);
        let mut frames = Vec::new();
        let mut off = 0usize;
        loop {
            let avail = self.buf.len() - off;
            // Magic is rejected as soon as four bytes are known, without
            // waiting for the rest of the header.
            if avail >= 4 && self.buf[off..off + 4] != MAGIC {
                return Err(self.fail(CodecError::BadMagic));
            }
            if avail < HEADER_LEN {
                break;
            }
            let h = &self.buf[off..off + HEADER_LEN];
            let kind = h[4];
            let flags = h[5];
            let reserved = u16::from_be_bytes([h[6], h[7]]);
            let len = u32::from_be_bytes([h[8], h[9], h[10], h[11]]);
            if reserved != 0 {
                return Err(self.fail(CodecError::BadReserved(reserved)));
            }
            if len as usize > self.max_payload {
                return Err(self.fail(CodecError::TooLarge {
                    len,
                    max: self.max_payload,
                }));
            }
            let total = HEADER_LEN + len as usize;
            if self.buf.len() - off < total {
                break;
            }
            frames.push(Frame {
                header: Header { kind, flags },
                payload: self.buf[off + HEADER_LEN..off + total].to_vec(),
            });
            off += total;
        }
        if off > 0 {
            self.buf.drain(..off);
        }
        Ok(frames)
    }

    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    pub fn finish(&self) -> Result<(), CodecError> {
        if let Some(e) = &self.poisoned {
            return Err(e.clone());
        }
        if self.buf.is_empty() {
            Ok(())
        } else {
            Err(CodecError::Truncated)
        }
    }

    fn fail(&mut self, e: CodecError) -> CodecError {
        self.buf.clear();
        self.poisoned = Some(e.clone());
        e
    }
}
