use bench_util::{check_eq, section};
use t16_frame_codec::*;

fn frame(kind: u8, flags: u8, payload: &[u8]) -> Frame {
    Frame {
        header: Header { kind, flags },
        payload: payload.to_vec(),
    }
}

fn main() {
    bench_util::budget(24);
    section("length prefixed framing across arbitrary chunk boundaries", 24,
        20,
        || {
            let enc = Encoder::new(64);
            let a = frame(7, 0b1010_0001, b"hello");
            let bytes = enc.encode(&a).unwrap();
            check_eq!(HEADER_LEN, 12);
            check_eq!(bytes.len(), 17);
            check_eq!(&bytes[..4], MAGIC.as_slice());
            check_eq!(bytes[4], 7);
            check_eq!(bytes[5], 0b1010_0001);
            check_eq!(&bytes[6..8], &[0, 0]);
            check_eq!(&bytes[8..12], &[0, 0, 0, 5]);

            check_eq!(
                enc.encode(&frame(0, 0, &[0u8; 65])),
                Err(CodecError::TooLarge { len: 65, max: 64 })
            );
            check_eq!(enc.encode(&frame(0, 0, &[0u8; 64])).unwrap().len(), 76);

            let mut d = Decoder::new(64);
            check_eq!(d.feed(&[]).unwrap(), Vec::new());
            check_eq!(d.feed(&bytes[..3]).unwrap(), Vec::new());
            check_eq!(d.buffered(), 3);
            check_eq!(d.feed(&bytes[3..]).unwrap(), vec![a.clone()]);
            check_eq!(d.buffered(), 0);
            check_eq!(d.finish(), Ok(()));

            let b = frame(1, 0, b"xyz");
            let a_bytes = enc.encode(&a).unwrap();
            let b_bytes = enc.encode(&b).unwrap();
            let mut stream = a_bytes.clone();
            stream.extend(&b_bytes);
            let cut = a_bytes.len() + HEADER_LEN + 1;
            let mut d = Decoder::new(64);
            check_eq!(d.feed(&stream[..cut]).unwrap(), vec![a.clone()]);
            check_eq!(d.buffered(), HEADER_LEN + 1);
            check_eq!(d.finish(), Err(CodecError::Truncated));

            let mut d = Decoder::new(64);
            check_eq!(d.feed(b"XXXX"), Err(CodecError::BadMagic));
            check_eq!(d.buffered(), 0);
            check_eq!(d.feed(&bytes), Err(CodecError::BadMagic));
            check_eq!(d.finish(), Err(CodecError::BadMagic));

            let mut bad = enc.encode(&b).unwrap();
            bad[6] = 1;
            check_eq!(
                Decoder::new(64).feed(&bad),
                Err(CodecError::BadReserved(256))
            );

            let mut declared = enc.encode(&frame(0, 0, b"")).unwrap();
            declared[8..12].copy_from_slice(&65u32.to_be_bytes());
            check_eq!(
                Decoder::new(64).feed(&declared),
                Err(CodecError::TooLarge { len: 65, max: 64 })
            );
        },
    );
    bench_util::finish();
}
