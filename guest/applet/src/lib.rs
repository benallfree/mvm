#![no_std]
//! A small applet that pulls in real core machinery: formatting, memcpy, and a
//! bounds-check panic path.

use core::fmt::Write;

struct Buf<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl Write for Buf<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        if end > self.out.len() {
            return Err(core::fmt::Error);
        }
        self.out[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Formats a reading into `out`, copies `src` after it (memcpy), and reads
/// `src[idx]` (bounds-checked: an out-of-range idx panics).
///
/// # Safety
/// `out`/`src` must be valid for `out_len`/`src_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn applet_entry(out: *mut u8, out_len: usize, src: *const u8, src_len: usize, idx: usize) -> u32 {
    let out = core::slice::from_raw_parts_mut(out, out_len);
    let src = core::slice::from_raw_parts(src, src_len);
    let picked = src[idx];
    let mut w = Buf { out, len: 0 };
    let _ = write!(w, "sensor {} = {:>5}.{:02} ({:#x})", idx, picked as u32 * 7, picked % 100, picked);
    let n = w.len;
    let rest = &mut w.out[n..];
    let k = rest.len().min(src.len());
    rest[..k].copy_from_slice(&src[..k]);
    (n + k) as u32
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
