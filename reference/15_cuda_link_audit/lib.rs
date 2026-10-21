#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuditError {
    MissingCudaLink,
    WrongCudaLinkOrder,
    MissingDynamicLibrary(String),
}
pub fn plan_link(libtorch_dir: &str, cuda: bool) -> Vec<String> {
    let mut plan = vec![format!("cargo:rustc-link-search=native={libtorch_dir}")];
    if cuda {
        plan.extend([
            "cargo:rustc-link-arg=-Wl,--no-as-needed".into(),
            "cargo:rustc-link-arg=-ltorch_cuda".into(),
            "cargo:rustc-link-arg=-Wl,--as-needed".into(),
        ]);
    }
    plan
}
pub fn audit_link(cuda: bool, directives: &[String], needed: &[String]) -> Result<(), AuditError> {
    if !cuda {
        return Ok(());
    }
    let no = directives
        .iter()
        .position(|s| s == "cargo:rustc-link-arg=-Wl,--no-as-needed");
    let lib = directives
        .iter()
        .position(|s| s == "cargo:rustc-link-arg=-ltorch_cuda");
    let yes = directives
        .iter()
        .position(|s| s == "cargo:rustc-link-arg=-Wl,--as-needed");
    let (Some(no), Some(lib), Some(yes)) = (no, lib, yes) else {
        return Err(AuditError::MissingCudaLink);
    };
    if !(no < lib && lib < yes) {
        return Err(AuditError::WrongCudaLinkOrder);
    }
    for expected in [
        "libtorch_cuda.so",
        "libc10_cuda.so",
        "libcudart.so",
        "libcudnn.so",
    ] {
        if !needed.iter().any(|actual| actual.starts_with(expected)) {
            return Err(AuditError::MissingDynamicLibrary(
                expected.trim_end_matches(".so").into(),
            ));
        }
    }
    Ok(())
}
