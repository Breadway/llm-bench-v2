use bench_util::{check, check_eq, section};
use t13_autodiff_lifetime::*;
fn main() {
    bench_util::budget(8);
    section(
        "load tracked weights and release training graphs",
        8,
        10,
        || {
            let mut p = Parameter::tracked(vec![1.0, 2.0]);
            p.load_values(vec![3.0, 4.0]);
            check!(p.requires_grad());
            check_eq!(p.values(), &[3.0, 4.0][..]);
            let before = p.values().to_vec();
            let mut engine = Engine::new();
            let mut out = engine.forward_train(&[2.0, 1.0], &p);
            check_eq!(engine.retained_graphs(), 1);
            check!(engine.backward(&mut out, &mut p, 0.1));
            check_eq!(engine.retained_graphs(), 0);
            check!(p.values()[0] < before[0]);
            let _ = engine.forward_eval(&[2.0, 1.0], &p);
            check_eq!(engine.retained_graphs(), 0);
            check!(!engine.backward(&mut out, &mut p, 0.1));
        },
    );
    bench_util::finish();
}
