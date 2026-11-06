//! Round-seven tests as a Thumb image for the Unicorn harness
//! (meshops/run_unicorn.py). Output goes to `MO_CONSOLE`; the harness reads it.
//! `mo_mark` brackets every measured section: the harness records its own
//! cycle estimate each time it is called.
#![no_std]
#![no_main]

use core::ptr::addr_of_mut;

#[no_mangle]
pub static mut MO_CONSOLE: [u8; 16384] = [0; 16384];
#[no_mangle]
pub static mut MO_CONSOLE_LEN: u32 = 0;
static mut TICKS: u32 = 0;
static mut MARKS: u32 = 0;

struct Out;
impl core::fmt::Write for Out {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        unsafe {
            let buf = &mut *addr_of_mut!(MO_CONSOLE);
            for &b in s.as_bytes() {
                if (MO_CONSOLE_LEN as usize) < buf.len() {
                    buf[MO_CONSOLE_LEN as usize] = b;
                    MO_CONSOLE_LEN += 1;
                }
            }
        }
        Ok(())
    }
}

/// The harness hooks this address. Returns a mark counter, not cycles.
#[no_mangle]
#[inline(never)]
pub extern "C" fn mo_mark() -> u32 {
    unsafe {
        MARKS += 1;
        MARKS
    }
}

fn cycles() -> u32 {
    mo_mark()
}

/// kernel.timer.now.v1 under Unicorn: a tick per read (monotonic, not time).
fn now() -> u32 {
    unsafe {
        TICKS += 1;
        TICKS
    }
}

/// Calls per cost loop: Unicorn hooks every host instruction, so fewer than on the chips.
pub const N_UNICORN: u32 = 1000;

/// Round 7d: the harness raises the driver "interrupt" (calls `mo_irq` at an
/// instruction boundary, registers saved and restored around it) every
/// 64,000 estimated cycles (1 ms at 64 MHz) while this is nonzero.
#[no_mangle]
pub static mut MO_IRQ_ON: u32 = 0;

/// The driver interrupt handler: one fake-driver tick.
#[no_mangle]
#[inline(never)]
pub extern "C" fn mo_irq() {
    mvm_meshops_tests::asynct::driver_tick();
}

/// Idle (the WFI): the harness skips ahead to the next interrupt.
#[no_mangle]
#[inline(never)]
pub extern "C" fn mo_idle() {
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(MO_IRQ_ON)) };
}

struct Unicorn;
impl mvm_meshops_tests::asynct::Platform for Unicorn {
    async fn wait(&mut self) {
        mo_idle();
    }
}

#[no_mangle]
pub extern "C" fn mo_run() -> u32 {
    let ok = mvm_meshops_tests::run_all("unicorn", &mut Out, now, cycles, N_UNICORN);
    unsafe { core::ptr::write_volatile(core::ptr::addr_of_mut!(MO_IRQ_ON), 1) };
    let ok2 = mvm_meshops_tests::asynct::block_on(mvm_meshops_tests::asynct::run_async(
        "unicorn",
        &mut Out,
        &mut Unicorn,
        now,
        cycles,
        N_UNICORN,
        false,
    ));
    unsafe { core::ptr::write_volatile(core::ptr::addr_of_mut!(MO_IRQ_ON), 0) };
    (ok && ok2) as u32
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
