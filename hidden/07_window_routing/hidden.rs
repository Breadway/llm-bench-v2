use bench_util::{check, check_eq, section};
use t07_window_routing::*;

fn main() {
    bench_util::budget(74);
    section(
        "stale output pins, parent inheritance and multi-output hotplug",
        74,
        10,
        || {
            let mut r = OutputRouter::new(&["A", "B"], Some("A"));
            check!(r.pin("parent", "B"));
            check!(r.pin("child", "A"));
            check_eq!(r.route("child", Some("parent"), true), Some("A".into()));
            r.set_outputs(&["B"]);
            check_eq!(r.route("child", Some("parent"), true), Some("B".into()));
            check!(r.set_focused(Some("B")));
            check!(r.bind_auto("ordinary"));
            check_eq!(r.route("ordinary", None, true), Some("B".into()));
            r.set_outputs(&[]);
            check_eq!(r.route("ordinary", None, true), None);
            check_eq!(r.route("child", Some("parent"), false), None);
            check!(!r.pin_focused_output("unfocused"));

            let names = (0..32).map(|i| format!("OUT-{i}")).collect::<Vec<_>>();
            let refs = names.iter().map(String::as_str).collect::<Vec<_>>();
            let mut many = OutputRouter::new(&refs, Some("OUT-0"));
            for (i, name) in names.iter().enumerate() {
                let surface = format!("surface-{i}");
                check!(many.pin(&surface, name));
                check_eq!(many.route(&surface, None, true), Some(name.clone()));
            }
        },
    );
    bench_util::finish();
}
