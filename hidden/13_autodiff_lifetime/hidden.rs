use bench_util::{check, check_eq, section};
use t13_autodiff_lifetime::*;
fn main() {
    bench_util::budget(1536);
    section(
        "repeated train backward and graph-free validation",
        1536,
        20,
        || {
            let mut p = Parameter::tracked(vec![1.0, 1.0, 1.0]);
            let mut engine = Engine::new();
            for i in 0..256 {
                p.load_values(vec![i as f32 + 1.0; 3]);
                check!(p.requires_grad());
                let mut out = engine.forward_train(&[1.0, 2.0, 3.0], &p);
                check_eq!(engine.retained_graphs(), 1);
                check!(engine.backward(&mut out, &mut p, 0.001));
                check_eq!(engine.retained_graphs(), 0);
            }
            for i in 0..256 {
                p.load_values(vec![i as f32; 3]);
                let _ = engine.forward_eval(&[1.0, 2.0, 3.0], &p);
                check_eq!(engine.retained_graphs(), 0);
                check!(p.requires_grad());
            }
        },
    );
    bench_util::finish();
}
