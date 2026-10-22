# Incremental length prefixed frame codec

Implement `src/lib.rs` for a binary frame protocol that arrives in arbitrary chunks. The visible
test pins the API and the wire layout; `MAGIC` and `HEADER_LEN` are already declared.

A frame is a 12 byte header followed by the payload:

| bytes | field |
| ----- | ----- |
| 0..4  | `MAGIC` (`BRD1`) |
| 4     | `kind` |
| 5     | `flags` |
| 6..8  | reserved, big endian, must be zero |
| 8..12 | payload length, big endian, `u32` |

`Encoder::encode` writes a header and the payload. A payload longer than the encoder's
`max_payload`, or longer than `u32::MAX`, is `TooLarge { len, max }`; `len` is the payload length
clamped to `u32::MAX` and `max` is the encoder's limit. Nothing is written on error.

`Decoder::feed` accepts any byte slices, keeps an incomplete frame buffered, and returns the
frames that the call completed, in order. The declared length is checked against the decoder's
`max_payload` as soon as the header is available, before the payload is buffered. A bad magic is
reported once four bytes are available, a non-zero reserved field once the header is available.

Errors are sticky. After any error, `feed` and `finish` return that same error, `buffered` is 0,
and the buffer holds nothing. A call that detects an error returns `Err` and no frames, even if
it completed some frames earlier in the same call; those frames are dropped with the buffer.

`Decoder::buffered` is the number of bytes of an incomplete frame currently held: 0 when the
decoder is idle, poisoned, or exactly on a frame boundary. `finish` reports `Truncated` when
bytes of an incomplete frame are held and `Ok(())` otherwise. `finish` does not change the
decoder's state, and a `Truncated` result does not poison it: a later `feed` can still complete
the frame.

Payloads may be empty and may be exactly `max_payload` bytes long. Do not assume that a feed
starts or ends on a frame boundary, and do not allocate a buffer proportional to a declared
length that exceeds the limit. Decoding must stay linear in the bytes fed: feeding half a
megabyte one byte at a time is a normal case, so do not copy or rescan the whole buffer on
every call.
