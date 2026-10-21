use bench_util::{check, check_eq, section};
use t09_bread_client::*;

fn main() {
    bench_util::budget(32);
    section(
        "reconnect state, replayed subscriptions and command gates",
        32,
        10,
        || {
            let mut c = ClientCore::new("clip").unwrap();
            check_eq!(ClientCore::new("Clip").err(), Some(ClientError::InvalidApp));
            check_eq!(c.subscribe("bread.clip.copied"), Ok(None));
            check_eq!(c.subscribe("bread.clip.copied"), Ok(None));
            check_eq!(
                c.subscribe("bread..copied"),
                Err(ClientError::InvalidPattern)
            );
            check_eq!(c.poll_connect(900), Some(ConnectAttempt(0)));
            check_eq!(c.poll_connect(901), None);
            check_eq!(
                c.connected(1, Some("1.7")),
                Err(ClientError::InvalidApiVersion)
            );
            check_eq!(
                c.connected(1, None),
                Ok(vec![Frame::Subscribe("bread.clip.copied".into())])
            );
            check_eq!(
                c.connected(2, Some("1.7.0")),
                Err(ClientError::NotConnecting)
            );
            check!(c.is_connected());
            check_eq!(c.generation(), Some(1));
            check_eq!(
                c.command("box", "launch", b"x"),
                Err(ClientError::UnsupportedCommands)
            );
            check_eq!(
                c.emit("bread.clip.copied", &[0, 255]),
                Ok(Event {
                    id: 1,
                    topic: "bread.clip.copied".into(),
                    source: Some("clip".into()),
                    payload: vec![0, 255],
                })
            );
            check_eq!(
                c.emit("bread.paper.changed", b"x"),
                Err(ClientError::InvalidTopic)
            );
            check_eq!(c.disconnected(1_000), Ok(1_250));
            check_eq!(c.disconnected(1_100), Err(ClientError::NotConnecting));
            check!(!c.is_connected());
            check_eq!(c.poll_connect(1_249), None);
            check_eq!(c.poll_connect(1_250), Some(ConnectAttempt(1)));
            check_eq!(c.poll_connect(1_250), None);
            check_eq!(
                c.connected(1, Some("1.7.0")),
                Err(ClientError::StaleGeneration)
            );
            check_eq!(
                c.connected(2, Some("1.6.9")),
                Ok(vec![Frame::Subscribe("bread.clip.copied".into())])
            );
            check_eq!(
                c.command("box", "launch", b"x"),
                Err(ClientError::UnsupportedCommands)
            );
            check_eq!(c.disconnected(2_000), Ok(2_250));
            check_eq!(c.poll_connect(2_249), None);
            check_eq!(c.poll_connect(2_250), Some(ConnectAttempt(1)));
            check_eq!(
                c.connected(3, Some("1.7.0")),
                Ok(vec![Frame::Subscribe("bread.clip.copied".into())])
            );
            check_eq!(
                c.command("box", "launch", &[7]),
                Ok(Event {
                    id: 2,
                    topic: "bread.command.box.launch".into(),
                    source: None,
                    payload: vec![7],
                })
            );
            check_eq!(
                c.command("unknown", "launch", b""),
                Err(ClientError::InvalidTarget)
            );
            check_eq!(
                c.unsubscribe("bread.clip.copied"),
                Some(Frame::Unsubscribe("bread.clip.copied".into()))
            );
            check_eq!(c.unsubscribe("bread.clip.copied"), None);
            check!(retry_delay_ms(8) == 30_000);
        },
    );
    bench_util::finish();
}
