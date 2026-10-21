use bench_util::{check, check_eq, section};
use t01_theme_runtime::*;

fn main() {
    bench_util::budget(279);
    section("theme token and app stylesheet precedence", 279, 10, || {
        check_eq!(Color::parse_hex("#abc"), Some(Color(170, 187, 204)));
        check_eq!(Color::parse_hex("#A0f"), Some(Color(170, 0, 255)));
        check_eq!(Color::parse_hex("#102030"), Some(Color(16, 32, 48)));
        check_eq!(Color::parse_hex("102030"), None);
        check_eq!(Color::parse_hex("#12"), None);
        check_eq!(Color::parse_hex("#gg0000"), None);
        for n in 0..=255 {
            check_eq!(
                Color::parse_hex(&format!("#{n:02X}0000")).map(|c| c.0),
                Some(n as u8)
            );
        }
        check!((Color(0, 0, 0).luminance() - 0.0).abs() < 1e-12);
        check!((Color(255, 255, 255).luminance() - 1.0).abs() < 1e-12);
        check!((Color(0, 0, 0).contrast(Color(255, 255, 255)) - 21.0).abs() < 1e-9);
        check_eq!(
            Color(10, 20, 30).contrast(Color(240, 230, 220)),
            Color(240, 230, 220).contrast(Color(10, 20, 30))
        );
        check_eq!(Color(0, 0, 0).readable_ink(), Color(255, 255, 255));
        check_eq!(Color(255, 255, 255).readable_ink(), Color(0, 0, 0));

        let mut cascade = CssCascade::new();
        check!(
            cascade
                .load(
                    Origin::Shared,
                    ".bar { color: white; padding: 4px; } .osd, .notif { color: red; }"
                )
                .is_ok()
        );
        check!(cascade.load(Origin::App, ".bar { color: blue; }").is_ok());
        check_eq!(cascade.resolve(".bar", "color"), Some("blue"));
        check_eq!(cascade.resolve(".bar", "padding"), Some("4px"));
        check_eq!(cascade.resolve(".osd", "color"), Some("red"));
        check_eq!(cascade.resolve(".missing", "color"), None);
        check!(cascade.load(Origin::App, ".broken { color; }").is_err());
        check_eq!(cascade.resolve(".bar", "color"), Some("blue"));
        check_eq!(Color::parse_hex("#１２３"), None);
        check!(
            cascade
                .load(Origin::Shared, ".dup { color: red; color: green; }")
                .is_ok()
        );
        check_eq!(cascade.resolve(".dup", "color"), Some("green"));
    });
    bench_util::finish();
}
