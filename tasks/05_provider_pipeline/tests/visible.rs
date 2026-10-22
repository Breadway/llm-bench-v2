use bench_util::{check, check_eq, section};
use t05_provider_pipeline::*;

fn main() {
    bench_util::budget(17);
    section(
        "provider truth, shape cache and padded output",
        17,
        10,
        || {
            let states = [
                ProviderState {
                    backend: Backend::Cpu,
                    available: true,
                    registered: true,
                    detail: String::new(),
                },
                ProviderState {
                    backend: Backend::Migraphx,
                    available: true,
                    registered: true,
                    detail: String::new(),
                },
                ProviderState {
                    backend: Backend::Cuda,
                    available: true,
                    registered: false,
                    detail: "provider library missing".into(),
                },
            ];
            check_eq!(
                choose_backend(Backend::Migraphx, &states).selected,
                Backend::Migraphx
            );
            check_eq!(
                choose_backend(Backend::Migraphx, &states).fallback_reason,
                None
            );
            let fallback = choose_backend(Backend::Cuda, &states);
            check_eq!(fallback.selected, Backend::Cpu);
            check_eq!(
                fallback.fallback_reason.as_deref(),
                Some("provider library missing")
            );
            check_eq!(choose_backend(Backend::Cpu, &states).fallback_reason, None);
            let unconfigured = choose_backend(Backend::Cuda, &[]);
            check_eq!(
                unconfigured.fallback_reason.as_deref(),
                Some("Cuda provider was not configured")
            );
            let unavailable = [ProviderState {
                backend: Backend::Migraphx,
                available: false,
                registered: false,
                detail: String::new(),
            }];
            check_eq!(
                choose_backend(Backend::Migraphx, &unavailable).selected,
                Backend::Cpu
            );
            let choice = choose_backend(Backend::Migraphx, &unavailable);
            check_eq!(
                choice.fallback_reason.as_deref(),
                Some("Migraphx provider is unavailable")
            );

            let mut cache = ShapeCache::default();
            check!(cache.remember(1, 128));
            check!(!cache.remember(1, 128));
            check!(cache.remember(2, 64));
            check_eq!(cache.len(), 2);

            let output = [1.0, 3.0, 5.0, 7.0, 100.0, 100.0];
            check_eq!(mean_pool(&output, 3, 2, &[1, 1, 0]), Some(vec![3.0, 5.0]));
            check_eq!(mean_pool(&output, 3, 2, &[1]), Some(vec![1.0, 3.0]));
            check_eq!(mean_pool(&output, 2, 2, &[0, 0]), Some(vec![0.0, 0.0]));
            check_eq!(mean_pool(&output, 2, 0, &[1]), None);
            check_eq!(mean_pool(&output[..3], 2, 2, &[1, 1]), None);
        },
    );
    bench_util::finish();
}
