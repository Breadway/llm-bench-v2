use bench_util::{check, check_eq, section};
use t17_event_replay::*;

fn main() {
    bench_util::budget(46);
    section("pattern grammar and matching", 15, 10, || {
        let c = Pattern::parse("a.+.c").unwrap();
        check!(c.matches("a.b.c"));
        check!(!c.matches("a.c"));
        check!(!c.matches("a.b.b.c"));
        let rest = Pattern::parse("a.#").unwrap();
        check!(rest.matches("a"));
        check!(rest.matches("a.b.c"));
        let exact = Pattern::parse("a.b").unwrap();
        check!(exact.matches("a.b"));
        check!(!exact.matches("a.b.c"));
        check!(Pattern::parse("#").unwrap().matches("anything"));
        check_eq!(
            Pattern::parse("a.#.c"),
            Err(PatternError::MultiWildcardNotLast)
        );
        check_eq!(Pattern::parse("a..b"), Err(PatternError::EmptySegment));
        check_eq!(Pattern::parse("A.b"), Err(PatternError::BadSegment));
        check_eq!(Pattern::parse(""), Err(PatternError::Empty));
        check!(valid_topic("dev.worker.log"));
        check!(!valid_topic("dev.+.log"));
        check!(!valid_topic(""));
    });
    section("publish, replay and delivery order", 16, 10, || {
        let mut bus = Bus::new(4);
        let a = bus.subscribe("dev.+.log").unwrap();
        let b = bus.subscribe("dev.#").unwrap();
        check_eq!(a, 1);
        check_eq!(b, 2);
        let (seq, deliveries) = bus.publish("dev.api.log", b"hi").unwrap();
        check_eq!(seq, 1);
        check_eq!(deliveries.len(), 2);
        check_eq!(deliveries[0].subscription, a);
        check_eq!(deliveries[1].subscription, b);
        check_eq!(deliveries[0].payload, b"hi".to_vec());
        check_eq!(bus.retained(), 1);
        check_eq!(bus.publish("dev.api", b"x").unwrap().1.len(), 1);
        check_eq!(bus.publish("prod.api.log", b"x").unwrap().1.len(), 0);
        check_eq!(bus.publish("dev..log", b"x"), Err(BusError::InvalidTopic));
        check_eq!(bus.publish("dev.+", b"x"), Err(BusError::InvalidTopic));
        let events = bus.replay_since(1).unwrap();
        check_eq!(events.len(), 2);
        check_eq!(events[0].seq, 2);
        check_eq!(events[1].topic, "prod.api.log");
        check_eq!(bus.replay_since(3).unwrap(), Vec::new());
    });
    section("removal leaves every other subscription intact", 10,
        10,
        || {
            let mut bus = Bus::new(8);
            let a = bus.subscribe("dev.+.log").unwrap();
            let b = bus.subscribe("dev.#").unwrap();
            let c = bus.subscribe("dev.api.log").unwrap();
            check_eq!(bus.active(), 3);
            check!(bus.unsubscribe(b));
            check_eq!(bus.active(), 2);
            check!(!bus.unsubscribe(b));
            let (_, deliveries) = bus.publish("dev.api.log", b"x").unwrap();
            check_eq!(deliveries.len(), 2);
            check_eq!(deliveries[0].subscription, a);
            check_eq!(deliveries[1].subscription, c);
            check!(bus.unsubscribe(a));
            check!(bus.unsubscribe(c));
            check_eq!(bus.active(), 0);
        },
    );
    section("retained history and replay gaps", 5, 10, || {
        let mut bus = Bus::new(2);
        for _ in 0..3 {
            bus.publish("a.b", b"x").unwrap();
        }
        check_eq!(bus.retained(), 2);
        check_eq!(bus.replay_since(0), Err(ReplayError::Gap { oldest: 2 }));
        check_eq!(bus.replay_since(1).unwrap().len(), 2);
        check_eq!(bus.replay_since(2).unwrap().len(), 1);
        check_eq!(bus.replay_since(3).unwrap().len(), 0);
    });
    bench_util::finish();
}
