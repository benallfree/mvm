//! Round-seven tests as a Thumb image for the Unicorn harness
//! (meshops/run_unicorn.py). Output goes to `MO_CONSOLE`; the harness reads it.
//! `mo_mark` brackets every measured section: the harness records its own
//! cycle estimate each time it is called.
#![no_std]
#![no_main]

use core::ptr::addr_of_mut;

#[no_mangle]
pub static mut MO_CONSOLE: [u8; 8192] = [0; 8192];
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

#[no_mangle]
pub extern "C" fn mo_run() -> u32 {
    mvm_meshops_tests::run_all("unicorn", &mut Out, now, cycles, N_UNICORN) as u32
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
