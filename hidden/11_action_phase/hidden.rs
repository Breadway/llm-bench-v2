use bench_util::{check, check_eq, section};
use t11_action_phase::*;
fn main() {
    bench_util::budget(2048);
    section(
        "bounded progress under repeating net-positive and mixed policies",
        2048,
        20,
        || {
            let mixed = [
                Action::new(usize::MAX, usize::MAX - 1),
                Action::new(0, 2),
                Action::END,
            ];
            for n in 0..512 {
                let cap = n % 31;
                let result =
                    run_phase(n % 19 + 6, 5, &mixed[..if n % 2 == 0 { 1 } else { 2 }], cap);
                check!(result.actions <= cap);
                check_eq!(
                    result.forced_end,
                    result.actions == cap && result.hand_size > 5
                );
            }
            for n in 0..512 {
                let cap = n % 23;
                let result = run_phase(8, 5, &[Action::new(2, 0)], cap);
                check_eq!(result.actions, cap);
                check!(result.forced_end);
            }
        },
    );
    bench_util::finish();
}
