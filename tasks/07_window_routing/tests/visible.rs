use bench_util::{check, check_eq, section};
use t07_window_routing::*;

fn main() {
    bench_util::budget(16);
    section(
        "explicit OSD pinning across focused outputs",
        16,
        10,
        || {
            let mut r = OutputRouter::new(&["DP-1", "HDMI-A-1"], Some("HDMI-A-1"));
            check_eq!(r.route("main", None, false), Some("HDMI-A-1".into()));
            check_eq!(r.route("osd", None, true), None);
            check!(!r.pin("osd", "missing"));
            check!(r.pin("osd", "HDMI-A-1"));
            check_eq!(r.route("osd", None, true), Some("HDMI-A-1".into()));
            check!(r.set_focused(Some("DP-1")));
            check_eq!(r.route("main", None, false), Some("DP-1".into()));
            check_eq!(r.route("osd", None, true), Some("HDMI-A-1".into()));
            check!(!r.bind_auto("osd"));
            check!(r.pin_focused_output("notif"));
            check_eq!(r.route("notif", None, true), Some("DP-1".into()));
            check_eq!(
                r.route("transient", Some("osd"), true),
                Some("HDMI-A-1".into())
            );
            r.set_outputs(&["DP-1"]);
            check_eq!(r.route("osd", None, true), None);
            check!(!r.set_focused(Some("HDMI-A-1")));
            check_eq!(r.route("main", None, false), Some("DP-1".into()));
            r.unpin("notif");
            check_eq!(r.route("notif", None, false), Some("DP-1".into()));
        },
    );
    bench_util::finish();
}
