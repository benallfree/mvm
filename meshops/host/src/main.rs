//! The mesh-op tests on the x86 host: rounds 7-7c synchronously, then the
//! round-7d async tests with the fake drivers ticking on their own thread
//! (true concurrency: the "interrupt" runs while guest code does).
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

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

/// Sleep a little: the drivers' thread pushes events meanwhile.
struct Host;
impl mvm_meshops_tests::asynct::Platform for Host {
    async fn wait(&mut self) {
        std::thread::sleep(Duration::from_micros(100));
    }
}

static STOP: AtomicBool = AtomicBool::new(false);

fn main() {
    let ok = mvm_meshops_tests::run_all(
        "host",
        &mut Stdout,
        micros,
        nanos,
        mvm_meshops_tests::N_CALLS,
    );
    // The driver "interrupt": a thread ticking every millisecond of wall time.
    let drivers = std::thread::spawn(|| {
        let start = Instant::now();
        let mut n = 0u64;
        while !STOP.load(Ordering::Relaxed) {
            n += 1;
            let next = start + Duration::from_millis(n);
            if let Some(d) = next.checked_duration_since(Instant::now()) {
                std::thread::sleep(d);
            }
            mvm_meshops_tests::asynct::driver_tick();
        }
    });
    let ok2 = mvm_meshops_tests::asynct::block_on(mvm_meshops_tests::asynct::run_async(
        "host",
        &mut Stdout,
        &mut Host,
        micros,
        nanos,
        mvm_meshops_tests::N_CALLS,
        false,
    ));
    STOP.store(true, Ordering::Relaxed);
    let _ = drivers.join();
    std::process::exit(if ok && ok2 { 0 } else { 1 });
}
