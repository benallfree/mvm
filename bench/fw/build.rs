fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-search={dir}");
    println!("cargo:rustc-link-arg-bins=-Tbench.ld");
    // Absolute path: if the checkout moves (e.g. mounted at /work in Docker),
    // the old path no longer exists, so cargo reruns this script instead of
    // reusing a cached link-search path that points at the old location.
    println!("cargo:rerun-if-changed={dir}/bench.ld");
    println!("cargo:rerun-if-changed=../guest-image/kernels.bin");
}
