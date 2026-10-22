use bench_util::{check_eq, section};
use t16_frame_codec::*;

fn frame(kind: u8, flags: u8, payload: &[u8]) -> Frame {
    Frame {
        header: Header { kind, flags },
        payload: payload.to_vec(),
    }
}

fn next(state: &mut u64) -> usize {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (*state >> 33) as usize
}

fn main() {
    bench_util::budget(577);
    section("every split of a three frame stream reassembles", 61,
        30,
        || {
            let enc = Encoder::new(64);
            let frames = vec![
                frame(1, 0, b""),
                frame(2, 3, b"hello"),
                frame(3, 0, &[7u8; 17]),
            ];
            let mut stream = Vec::new();
            for f in &frames {
                stream.extend(enc.encode(f).unwrap());
            }
            for cut in 0..=stream.len() {
                let mut d = Decoder::new(64);
                let mut got = d.feed(&stream[..cut]).unwrap();
                got.extend(d.feed(&stream[cut..]).unwrap());
                check_eq!(got, frames);
            }
            let mut d = Decoder::new(64);
            let mut got = Vec::new();
            for b in &stream {
                got.extend(d.feed(std::slice::from_ref(b)).unwrap());
            }
            check_eq!(got, frames);
            check_eq!(d.buffered(), 0);
        },
    );
    section("seeded random streams and chunkings", 192, 30, || {
        let mut st = 0x9E37_79B9_7F4A_7C15u64;
        for _case in 0..64 {
            let count = 1 + next(&mut st) % 4;
            let mut frames = Vec::new();
            let mut stream = Vec::new();
            for _ in 0..count {
                let len = next(&mut st) % 41;
                let kind = next(&mut st) as u8;
                let flags = next(&mut st) as u8;
                let payload: Vec<u8> = (0..len).map(|i| (next(&mut st) ^ i) as u8).collect();
                let f = frame(kind, flags, &payload);
                stream.extend(Encoder::new(64).encode(&f).unwrap());
                frames.push(f);
            }
            let mut d = Decoder::new(64);
            let mut got = Vec::new();
            let mut off = 0;
            while off < stream.len() {
                let end = (off + 1 + next(&mut st) % 13).min(stream.len());
                got.extend(d.feed(&stream[off..end]).unwrap());
                off = end;
            }
            check_eq!(got, frames);
            check_eq!(d.buffered(), 0);
            check_eq!(d.finish(), Ok(()));
        }
    });
    section("error paths are sticky and drop earlier frames", 54,
        20,
        || {
            let enc = Encoder::new(64);
            let valid = enc.encode(&frame(9, 4, b"abc")).unwrap();
            for cut in 0..HEADER_LEN {
                let mut d = Decoder::new(64);
                check_eq!(d.feed(&valid[..cut]), Ok(Vec::new()));
                check_eq!(d.buffered(), cut);
                check_eq!(
                    d.finish(),
                    if cut == 0 {
                        Ok(())
                    } else {
                        Err(CodecError::Truncated)
                    }
                );
            }

            let mut d = Decoder::new(4);
            check_eq!(d.feed(&valid[..8]), Ok(Vec::new()));
            check_eq!(d.finish(), Err(CodecError::Truncated));
            check_eq!(d.feed(&valid[8..]).unwrap(), vec![frame(9, 4, b"abc")]);
            check_eq!(d.finish(), Ok(()));

            let mut d = Decoder::new(64);
            check_eq!(d.feed(b"BRD"), Ok(Vec::new()));
            check_eq!(d.feed(b"X"), Err(CodecError::BadMagic));
            check_eq!(d.buffered(), 0);
            check_eq!(d.feed(&valid), Err(CodecError::BadMagic));
            check_eq!(d.finish(), Err(CodecError::BadMagic));

            let mut bad = enc.encode(&frame(0, 0, b"z")).unwrap();
            bad[7] = 2;
            let mut d = Decoder::new(64);
            check_eq!(d.feed(&bad), Err(CodecError::BadReserved(2)));
            check_eq!(d.feed(&valid), Err(CodecError::BadReserved(2)));
            check_eq!(d.buffered(), 0);

            let mut declared = enc.encode(&frame(0, 0, b"")).unwrap();
            declared[8..12].copy_from_slice(&1000u32.to_be_bytes());
            let mut d = Decoder::new(64);
            check_eq!(
                d.feed(&declared),
                Err(CodecError::TooLarge { len: 1000, max: 64 })
            );
            check_eq!(d.buffered(), 0);
            check_eq!(d.finish(), Err(CodecError::TooLarge { len: 1000, max: 64 }));

            let mut stream = enc.encode(&frame(1, 0, b"a")).unwrap();
            stream.extend_from_slice(b"XXXX");
            let mut d = Decoder::new(64);
            check_eq!(d.feed(&stream), Err(CodecError::BadMagic));
            check_eq!(d.buffered(), 0);
            check_eq!(d.feed(&valid), Err(CodecError::BadMagic));
        },
    );
    section("declared lengths, reserved fields and kind ranges", 267,
        20,
        || {
            for k in 0..256u32 {
                let kind = k as u8;
                let f = frame(kind, kind, &[kind; 3]);
                let bytes = Encoder::new(64).encode(&f).unwrap();
                check_eq!(Decoder::new(64).feed(&bytes).unwrap(), vec![f]);
            }
            for r in [1u16, 2, 255, 256, 65535] {
                let mut bytes = Encoder::new(64).encode(&frame(0, 0, b"")).unwrap();
                bytes[6..8].copy_from_slice(&r.to_be_bytes());
                check_eq!(
                    Decoder::new(64).feed(&bytes),
                    Err(CodecError::BadReserved(r))
                );
            }

            let enc0 = Encoder::new(0);
            check_eq!(enc0.encode(&frame(0, 0, b"")).unwrap().len(), HEADER_LEN);
            check_eq!(
                enc0.encode(&frame(0, 0, b"x")),
                Err(CodecError::TooLarge { len: 1, max: 0 })
            );
            let mut d = Decoder::new(0);
            check_eq!(
                d.feed(&enc0.encode(&frame(0, 0, b"")).unwrap()).unwrap(),
                vec![frame(0, 0, b"")]
            );
            check_eq!(d.finish(), Ok(()));

            let enc = Encoder::new(64);
            let big = frame(0, 0, &[9u8; 64]);
            check_eq!(
                Decoder::new(64).feed(&enc.encode(&big).unwrap()).unwrap(),
                vec![big]
            );

            let mut bytes = enc.encode(&frame(0, 0, b"")).unwrap();
            bytes[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
            check_eq!(
                Decoder::new(64).feed(&bytes),
                Err(CodecError::TooLarge {
                    len: u32::MAX,
                    max: 64
                })
            );
        },
    );
    section("a large frame fed one byte at a time stays linear", 3, 25, || {
        let enc = Encoder::new(1 << 20);
        let big = frame(5, 0, &vec![0xA5u8; 512 * 1024]);
        let bytes = enc.encode(&big).unwrap();
        let mut d = Decoder::new(1 << 20);
        let mut got = Vec::new();
        for b in &bytes {
            got.extend(d.feed(std::slice::from_ref(b)).unwrap());
        }
        check_eq!(got, vec![big]);
        check_eq!(d.buffered(), 0);
        check_eq!(d.finish(), Ok(()));
    });
    bench_util::finish();
}
