use bench_util::{check, check_eq, section};
use t17_event_replay::*;

/// Independent matcher used as the oracle for the exhaustive section.
fn oracle(pattern: &str, topic: &str) -> bool {
    fn go(p: &[&str], t: &[&str]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some((&"#", rest)) => rest.is_empty(),
            Some((&"+", rest)) => !t.is_empty() && go(rest, &t[1..]),
            Some((literal, rest)) => t.first() == Some(literal) && go(rest, &t[1..]),
        }
    }
    let p: Vec<&str> = pattern.split('.').collect();
    let t: Vec<&str> = topic.split('.').collect();
    go(&p, &t)
}

fn permutations(n: usize) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    let mut a: Vec<usize> = (0..n).collect();
    let mut counters = vec![0usize; n];
    out.push(a.clone());
    let mut i = 0;
    while i < n {
        if counters[i] < i {
            if i % 2 == 0 {
                a.swap(0, i);
            } else {
                a.swap(counters[i], i);
            }
            out.push(a.clone());
            counters[i] += 1;
            i = 0;
        } else {
            counters[i] = 0;
            i += 1;
        }
    }
    out
}

fn main() {
    bench_util::budget(681);
    section("exhaustive pattern matching against the grammar", 406,
        30,
        || {
            let atoms = ["a", "+"];
            let mut patterns = Vec::new();
            let mut base = Vec::new();
            for x in atoms {
                base.push(x.to_string());
                for y in atoms {
                    base.push(format!("{x}.{y}"));
                    for z in atoms {
                        base.push(format!("{x}.{y}.{z}"));
                    }
                }
            }
            patterns.extend(base.iter().cloned());
            for p in &base {
                patterns.push(format!("{p}.#"));
            }
            patterns.push("#".to_string());

            let labels = ["a", "b"];
            let mut topics = Vec::new();
            for x in labels {
                topics.push(x.to_string());
                for y in labels {
                    topics.push(format!("{x}.{y}"));
                    for z in labels {
                        topics.push(format!("{x}.{y}.{z}"));
                    }
                }
            }

            for p in &patterns {
                let parsed = Pattern::parse(p).unwrap();
                for t in &topics {
                    check_eq!(parsed.matches(t), oracle(p, t));
                }
            }
        },
    );
    section("replay window boundaries and gaps", 16, 20, || {
        let mut bus = Bus::new(3);
        for i in 1..=5u8 {
            check_eq!(bus.publish("a.b", &[i]).unwrap().0, i as u64);
        }
        check_eq!(bus.retained(), 3);
        check_eq!(bus.replay_since(0), Err(ReplayError::Gap { oldest: 3 }));
        check_eq!(
            bus.replay_since(2)
                .unwrap()
                .iter()
                .map(|e| e.seq)
                .collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
        check_eq!(bus.replay_since(2).unwrap()[0].payload, vec![3]);
        check_eq!(bus.replay_since(3).unwrap().len(), 2);
        check_eq!(bus.replay_since(5).unwrap().len(), 0);
        check_eq!(bus.replay_since(6).unwrap().len(), 0);

        let mut tiny = Bus::new(1);
        tiny.publish("a.b", b"1").unwrap();
        tiny.publish("a.b", b"2").unwrap();
        check_eq!(tiny.retained(), 1);
        check_eq!(tiny.replay_since(0), Err(ReplayError::Gap { oldest: 2 }));
        check_eq!(tiny.replay_since(1).unwrap().len(), 1);
        check_eq!(tiny.replay_since(2).unwrap().len(), 0);
    });
    section("every removal order leaves its survivors intact", 240,
        30,
        || {
            let patterns = ["a.#", "a.+.c", "a.b.c", "#"];
            for order in permutations(patterns.len()) {
                let mut bus = Bus::new(16);
                let ids: Vec<u64> = patterns.iter().map(|p| bus.subscribe(p).unwrap()).collect();
                check_eq!(ids, vec![1, 2, 3, 4]);
                let mut remaining = patterns.len();
                for &pick in &order {
                    check!(bus.unsubscribe(ids[pick]));
                    remaining -= 1;
                    let (_, deliveries) = bus.publish("a.b.c", b"x").unwrap();
                    check_eq!(deliveries.len(), remaining);
                }
                check_eq!(bus.active(), 0);
            }
        },
    );
    section("ids, rejected subscriptions and topic validation", 19,
        20,
        || {
            let mut bus = Bus::new(4);
            check_eq!(
                bus.subscribe("a.#.b"),
                Err(BusError::InvalidPattern(PatternError::MultiWildcardNotLast))
            );
            check_eq!(
                bus.subscribe(""),
                Err(BusError::InvalidPattern(PatternError::Empty))
            );
            check_eq!(
                bus.subscribe("a..b"),
                Err(BusError::InvalidPattern(PatternError::EmptySegment))
            );
            check_eq!(
                bus.subscribe("A.b"),
                Err(BusError::InvalidPattern(PatternError::BadSegment))
            );
            check_eq!(bus.subscribe("a.b").unwrap(), 1);
            check_eq!(bus.subscribe("a.b").unwrap(), 2);
            check_eq!(bus.unsubscribe(99), false);
            check_eq!(bus.unsubscribe(1), true);
            check_eq!(bus.unsubscribe(1), false);
            check_eq!(bus.active(), 1);
            check_eq!(bus.publish("", b"x"), Err(BusError::InvalidTopic));
            check_eq!(bus.publish("a.+.b", b"x"), Err(BusError::InvalidTopic));
            check_eq!(bus.publish("a.B", b"x"), Err(BusError::InvalidTopic));
            check_eq!(bus.retained(), 0);
            let (seq, deliveries) = bus.publish("a.b", b"").unwrap();
            check_eq!(seq, 1);
            check_eq!(deliveries.len(), 1);
            check_eq!(deliveries[0].payload, Vec::<u8>::new());

            let mut wild = Bus::new(2);
            wild.subscribe("#").unwrap();
            check_eq!(wild.publish("one", b"x").unwrap().1.len(), 1);
            check_eq!(wild.publish("one.two", b"x").unwrap().1.len(), 1);
        },
    );
    bench_util::finish();
}
