use bench_util::{check, check_eq, section};
use t11_action_phase::*;
fn main() {
    bench_util::budget(12);
    section("pathological action policies terminate", 12, 10, || {
        let draw = Action::new(2, 0);
        let result = run_phase(6, 5, &[draw], 7);
        check_eq!(result.actions, 7);
        check!(result.forced_end);
        check_eq!(result.hand_size, 20);
        let result = run_phase(6, 5, &[Action::new(1, 2)], 20);
        check_eq!(result.hand_size, 5);
        check_eq!(result.actions, 1);
        check!(!result.forced_end);
        let result = run_phase(9, 5, &[Action::new(0, 3)], 0);
        check_eq!(result.hand_size, 9);
        check_eq!(result.actions, 0);
        check!(result.forced_end);
        check_eq!(run_phase(4, 5, &[], 20).actions, 0);
        let ended = run_phase(3, 5, &[Action::END], 20);
        check_eq!(ended.actions, 0);
        check_eq!(ended.hand_size, 3);
    });
    bench_util::finish();
}
