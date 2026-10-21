use bench_util::{check, check_eq, section};
use t02_event_bus::*;

fn main() {
    bench_util::budget(1034);
    section("wildcard and bounded replay edge cases", 1034, 20, || {
        let mut b = EventBus::new();
        check_eq!(b.subscribe("bread.clip.**").unwrap(), 1);
        check_eq!(b.subscribe("bread.clip.*").unwrap(), 2);
        check!(b.subscribe("bread.*.copied.more").is_ok());
        check!(b.subscribe("bread.**.tail").is_ok());
        let deep = Event::new(5, Some("clip"), "bread.clip.copied.detail", b"").unwrap();
        check_eq!(b.publish(&deep).unwrap(), vec![1]);
        check_eq!(
            b.publish(&Event::new(6, Some("clip"), "bread.clip.copied", b"").unwrap())
                .unwrap(),
            vec![1, 2]
        );
        check_eq!(
            b.publish(&Event::new(7, Some("clip"), "bread.clip.tail", b"").unwrap())
                .unwrap(),
            vec![1, 2, 4]
        );
        check!(
            b.publish(&Event {
                id: 99,
                topic: "bread.power.on".into(),
                source: None,
                payload: vec![]
            })
            .is_err()
        );
        check!(
            b.publish(&Event::new(99, None, "bread.command.clip.clear", b"").unwrap())
                .is_ok()
        );
        for id in 100..1124 {
            let event = Event::new(id, Some("clip"), "bread.clip.copied", b"").unwrap();
            check!(b.publish(&event).is_ok());
        }
        check_eq!(b.publish(&deep).unwrap(), vec![1]);
    });
    bench_util::finish();
}
