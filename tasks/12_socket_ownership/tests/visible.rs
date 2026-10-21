use bench_util::{check, check_eq, section};
use t12_socket_ownership::*;
fn main() {
    bench_util::budget(8);
    section(
        "live path protection and generation-safe cleanup",
        8,
        10,
        || {
            let registry = Registry::new();
            let first = registry.bind("/run/breadmill.sock").unwrap();
            check!(matches!(
                registry.bind("/run/breadmill.sock"),
                Err(BindError::InUse)
            ));
            check!(registry.is_live("/run/breadmill.sock"));
            first.crash();
            check!(!registry.is_live("/run/breadmill.sock"));
            let second = registry.bind("/run/breadmill.sock").unwrap();
            check!(second.id() > first.id());
            drop(first);
            check!(registry.is_live("/run/breadmill.sock"));
            check_eq!(registry.owner("/run/breadmill.sock"), Some(second.id()));
            drop(second);
            check!(!registry.exists("/run/breadmill.sock"));
            let third = registry.bind("/run/breadmill.sock").unwrap();
            check!(third.id() > 1);
        },
    );
    bench_util::finish();
}
