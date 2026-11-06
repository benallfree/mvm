fn main() {
    for c in &bench_core::CASES {
        let mut buf = [0u8; 256];
        bench_core::input(c, &mut buf);
        let v = unsafe {
            if c.crc {
                kernels::crc16(buf.as_ptr(), c.len)
            } else {
                kernels::parse(buf.as_ptr(), c.len)
            }
        };
        println!("{} {v:#x}", c.name);
    }
}
