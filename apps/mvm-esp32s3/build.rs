//! Links esp-hal's `linkall.x`, as Espressif's project template does.

fn main() {
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}
