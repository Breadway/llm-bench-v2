use bench_util::{check, check_eq, section};
use t01_theme_runtime::*;

fn main() {
    bench_util::budget(24);
    section("theme reload edge cases", 24, 10, || {
        check_eq!(Color::parse_hex("#１２３"), None);
        let mut c = CssCascade::new();
        check!(c.load(Origin::App, "").is_ok());
        check!(c.load(Origin::Shared, ".empty { }").is_ok());
        check!(
            c.load(Origin::Shared, ".a { color: red; color: green }")
                .is_ok()
        );
        check_eq!(c.resolve(".a", "color"), Some("green"));
        check!(c.load(Origin::App, ".a { color: blue }").is_ok());
        check!(c.load(Origin::Shared, ".a { color: black }").is_ok());
        check_eq!(c.resolve(".a", "color"), Some("blue"));
        for n in 0..16 {
            let background = Color((n * 17) as u8, (255 - n * 17) as u8, (n * 9) as u8);
            check!(background.contrast(background.readable_ink()) >= 4.5);
        }
    });
    bench_util::finish();
}
