use bench_util::{check, check_eq, section};
use t15_cuda_link_audit::*;
fn main() {
    bench_util::budget(10);
    section(
        "ordered build directives and runtime CUDA dependency audit",
        10,
        10,
        || {
            let plan = plan_link("/opt/libtorch/lib", true);
            check_eq!(
                plan[0].as_str(),
                "cargo:rustc-link-search=native=/opt/libtorch/lib"
            );
            let no = plan
                .iter()
                .position(|s| s.ends_with("-Wl,--no-as-needed"))
                .unwrap();
            let lib = plan
                .iter()
                .position(|s| s.ends_with("-ltorch_cuda"))
                .unwrap();
            let yes = plan
                .iter()
                .position(|s| s.ends_with("-Wl,--as-needed"))
                .unwrap();
            check!(no < lib && lib < yes);
            let deps = [
                "libtorch_cuda.so.12".to_string(),
                "libc10_cuda.so.12".to_string(),
                "libcudart.so.12".to_string(),
                "libcudnn.so.9".to_string(),
            ];
            check_eq!(audit_link(true, &plan, &deps), Ok(()));
            check_eq!(
                audit_link(true, &plan[..2], &deps),
                Err(AuditError::MissingCudaLink)
            );
            let wrong = [
                plan[0].clone(),
                plan[2].clone(),
                plan[1].clone(),
                plan[3].clone(),
            ];
            check_eq!(
                audit_link(true, &wrong, &deps),
                Err(AuditError::WrongCudaLinkOrder)
            );
            check_eq!(
                audit_link(true, &plan, &deps[..3]),
                Err(AuditError::MissingDynamicLibrary("libcudnn".into()))
            );
            let cpu = plan_link("/opt/libtorch/lib", false);
            check_eq!(
                cpu,
                vec![String::from(
                    "cargo:rustc-link-search=native=/opt/libtorch/lib"
                )]
            );
            check!(audit_link(false, &cpu, &[]).is_ok());
            check!(audit_link(false, &[], &[]).is_ok());
            let missing_cudnn = [
                "libtorch_cuda.so".to_string(),
                "libc10_cuda.so".to_string(),
                "libcudart.so".to_string(),
                "libtorch_cpu.so".to_string(),
            ];
            check!(audit_link(true, &plan, &missing_cudnn).is_err());
        },
    );
    bench_util::finish();
}
