//! Puts `memory.x` on the linker search path for `cortex-m-rt`'s `link.x`.

use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
}
