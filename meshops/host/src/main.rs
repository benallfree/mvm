use std::sync::OnceLock;
use std::time::Instant;

static T0: OnceLock<Instant> = OnceLock::new();

fn micros() -> u32 {
    T0.get_or_init(Instant::now).elapsed().as_micros() as u32
}
fn nanos() -> u32 {
    T0.get_or_init(Instant::now).elapsed().as_nanos() as u32
}

struct Stdout;
impl std::fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        print!("{s}");
        Ok(())
    }
}

fn main() {
    let ok = mvm_meshops_tests::run_all(
        "host",
        &mut Stdout,
        micros,
        nanos,
        mvm_meshops_tests::N_CALLS,
    );
    std::process::exit(if ok { 0 } else { 1 });
}
