use bench_util::{check, check_eq, section};
use t09_bread_client::*;

fn main() {
    bench_util::budget(527);
    section(
        "retry saturation, event identity and reconnect replay",
        527,
        20,
        || {
            let mut c = ClientCore::new("clip").unwrap();
            check_eq!(c.subscribe("bread.**"), Ok(None));
            check_eq!(c.subscribe("bread.**"), Ok(None));
            check_eq!(c.unsubscribe("bread.missing.*"), None);
            check_eq!(c.poll_connect(0), Some(ConnectAttempt(0)));
            check_eq!(
                c.connected(1, Some("1.7.1")),
                Ok(vec![Frame::Subscribe("bread.**".into())])
            );
            for attempt in 1..=256u32 {
                let expected = if attempt >= 8 {
                    30_000
                } else {
                    250u64 << (attempt - 1)
                };
                check_eq!(retry_delay_ms(attempt), expected);
            }
            for i in 0..256u64 {
                let payload = i.to_le_bytes();
                check_eq!(
                    c.emit(&format!("bread.clip.item{i}"), &payload),
                    Ok(Event {
                        id: i + 1,
                        topic: format!("bread.clip.item{i}"),
                        source: Some("clip".into()),
                        payload: payload.to_vec(),
                    })
                );
            }
            check_eq!(c.disconnected(1_000), Ok(1_250));
            check_eq!(c.poll_connect(1_249), None);
            check_eq!(c.poll_connect(1_250), Some(ConnectAttempt(1)));
            check_eq!(
                c.connected(2, Some("1.7.0")),
                Ok(vec![Frame::Subscribe("bread.**".into())])
            );
            check_eq!(
                c.unsubscribe("bread.**"),
                Some(Frame::Unsubscribe("bread.**".into()))
            );
            check_eq!(c.disconnected(u64::MAX), Ok(u64::MAX));
            check_eq!(c.poll_connect(u64::MAX), Some(ConnectAttempt(1)));
            check_eq!(c.connected(3, None), Ok(Vec::new()));
            check_eq!(
                c.command("box", "launch", b""),
                Err(ClientError::UnsupportedCommands)
            );
            check!(c.is_connected());
        },
    );
    bench_util::finish();
}
