use bench_util::{check, check_eq, section};
use t05_provider_pipeline::*;

fn main() {
    bench_util::budget(73);
    section(
        "silent provider fallback and padded sequence shapes",
        73,
        10,
        || {
            let absent = [ProviderState {
                backend: Backend::Migraphx,
                available: false,
                registered: false,
                detail: String::new(),
            }];
            check_eq!(
                choose_backend(Backend::Migraphx, &absent).selected,
                Backend::Cpu
            );
            let unavailable = choose_backend(Backend::Migraphx, &absent);
            check_eq!(
                unavailable.fallback_reason.as_deref(),
                Some("Migraphx provider is unavailable")
            );
            check_eq!(choose_backend(Backend::Cuda, &[]).selected, Backend::Cpu);
            let mut cache = ShapeCache::default();
            check!(cache.is_empty());
            check!(cache.remember(1, 256));
            check!(cache.remember(2, 128));
            check_eq!(
                mean_pool(&[2.0, 4.0, 6.0, 8.0], 2, 2, &[2, 0]),
                Some(vec![2.0, 4.0])
            );
            check_eq!(mean_pool(&[], 0, 2, &[]), Some(vec![0.0, 0.0]));
            check_eq!(mean_pool(&[], usize::MAX, 2, &[]), None);
            for len in 1..=64 {
                let actual = len + 5;
                let mask = (0..len).map(|i| u8::from(i % 3 != 1)).collect::<Vec<_>>();
                let output = (0..actual * 2).map(|i| i as f32 + 1.0).collect::<Vec<_>>();
                let mut expected = [0.0f32; 2];
                let mut count = 0.0f32;
                for i in 0..len {
                    if mask[i] != 0 {
                        expected[0] += output[i * 2];
                        expected[1] += output[i * 2 + 1];
                        count += 1.0;
                    }
                }
                if count > 0.0 {
                    expected[0] /= count;
                    expected[1] /= count;
                }
                check_eq!(
                    mean_pool(&output, actual, 2, &mask),
                    Some(expected.to_vec())
                );
            }
        },
    );
    bench_util::finish();
}
