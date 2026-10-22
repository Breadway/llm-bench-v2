use bench_util::{check, check_eq, section};
use t15_cuda_link_audit::*;
fn main() {
    bench_util::budget(768);
    section(
        "versioned dependency names, missing libraries and linker flag permutations",
        768,
        20,
        || {
            for n in 0..256 {
                let plan = plan_link("/libtorch", true);
                let v = n % 20 + 1;
                let deps = [
                    format!("libtorch_cuda.so.{v}"),
                    format!("libc10_cuda.so.{v}"),
                    format!("libcudart.so.{v}"),
                    format!("libcudnn.so.{v}"),
                ];
                check_eq!(audit_link(true, &plan, &deps), Ok(()));
            }
            for n in 0..256 {
                let plan = plan_link("/libtorch", true);
                let all = [
                    "libtorch_cuda.so".to_string(),
                    "libc10_cuda.so".to_string(),
                    "libcudart.so".to_string(),
                    "libcudnn.so".to_string(),
                ];
                let deps = all
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != n % 4)
                    .map(|(_, s)| s.clone())
                    .collect::<Vec<_>>();
                check!(audit_link(true, &plan, &deps).is_err());
            }
            for _ in 0..256 {
                let mut plan = plan_link("/libtorch", true);
                let a = plan
                    .iter()
                    .position(|s| s.ends_with("-Wl,--no-as-needed"))
                    .unwrap();
                let b = plan
                    .iter()
                    .position(|s| s.ends_with("-ltorch_cuda"))
                    .unwrap();
                plan.swap(a, b);
                let deps = [
                    "libtorch_cuda.so".to_string(),
                    "libc10_cuda.so".to_string(),
                    "libcudart.so".to_string(),
                    "libcudnn.so".to_string(),
                ];
                check_eq!(
                    audit_link(true, &plan, &deps),
                    Err(AuditError::WrongCudaLinkOrder)
                );
            }
        },
    );
    bench_util::finish();
}
