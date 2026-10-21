use bench_util::{check, check_eq, section};
use t02_event_bus::*;

fn main() {
    bench_util::budget(15);
    section("namespace validation and delivery", 15, 10, || {
        check!(validate_topic(Some("clip"), "bread.clip.copied").is_ok());
        check_eq!(
            validate_topic(Some("bar"), "bread.clip.copied"),
            Err(EventError::InvalidSource)
        );
        check!(validate_topic(None, "bread.command.clip.clear").is_ok());
        check_eq!(
            validate_topic(Some("clip"), "bread.command.clip.clear"),
            Err(EventError::InvalidSource)
        );
        check_eq!(
            validate_topic(None, "bread.power.off"),
            Err(EventError::ReservedDomain)
        );
        check_eq!(
            validate_topic(Some("bread"), "bread.power.off"),
            Err(EventError::ReservedDomain)
        );
        check!(validate_topic(None, "bread.command.help.open").is_ok());
        check_eq!(
            validate_topic(None, "bread.command.unknown.run"),
            Err(EventError::UnknownCommandTarget)
        );
        check_eq!(
            validate_topic(Some("Clip"), "bread.clip.copied"),
            Err(EventError::InvalidSource)
        );

        let mut bus = EventBus::new();
        let one = bus.subscribe("bread.clip.*").unwrap_or(0);
        let two = bus.subscribe("bread.**").unwrap_or(0);
        let three = bus.subscribe("bread.clip.copied").unwrap_or(0);
        check_eq!((one, two, three), (1, 2, 3));
        let e = Event::new(77, Some("clip"), "bread.clip.copied", b"text/plain").unwrap();
        check_eq!(e.payload, b"text/plain".to_vec());
        check_eq!(bus.publish(&e).unwrap(), vec![one, two, three]);
        check_eq!(bus.publish(&e).unwrap(), Vec::<u64>::new());
        let forged = Event {
            id: 78,
            topic: "bread.power.off".into(),
            source: None,
            payload: vec![],
        };
        check_eq!(bus.publish(&forged), Err(EventError::ReservedDomain));
        let forged = Event {
            id: 79,
            topic: "bread.hyprland.changed".into(),
            source: None,
            payload: vec![],
        };
        check_eq!(bus.publish(&forged), Err(EventError::ReservedDomain));
    });
    bench_util::finish();
}
