fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-search={dir}");
    println!("cargo:rustc-link-arg-bins=-Tbench.ld");
    println!("cargo:rerun-if-changed=bench.ld");
    println!("cargo:rerun-if-changed=../guest-image/kernels.bin");
}
