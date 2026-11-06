#![no_std]
//! The MVM kernel's mesh-op layer (round seven). See MESHOPS.md.
//!
//! - [`Registry`]: name hash -> entry. Entries keep the full name: the hash is
//!   only an index, and names are what get compared, so a crafted FNV
//!   collision can't shadow an op. Entries never move once added, so a
//!   resolved import can hold an entry index (a "live binding").
//! - [`Kernel::load`]: reads an applet's import/export tables from its image,
//!   checks them, registers its exports, and resolves its imports into a slot table.
//! - [`Kernel::call`]: runs guest code; each `ecall` looks up the slot (no
//!   hashing, no name compare) and runs the provider, native or guest. Guest
//!   providers re-enter the interpreter with a fresh register file, one level deeper.

use interp::fast::{self, Cpu, Stop};
use interp::{Fault, Mem};

/// FNV-1a 64 over the full versioned name, e.g. `kernel.uart.write.v1`.
pub const fn fnv1a64(s: &str) -> u64 {
    fnv1a64_bytes(s.as_bytes())
}

pub const fn fnv1a64_bytes(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < b.len() {
        h ^= b[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

pub const UART_WRITE: u64 = fnv1a64("kernel.uart.write.v1");
pub const TIMER_NOW: u64 = fnv1a64("kernel.timer.now.v1");
pub const NOP: u64 = fnv1a64("kernel.nop.v1");

/// Guest-visible register numbers of the call ABI.
pub const REG_SLOT: usize = 17; // a7
pub const REG_ARG0: usize = 10; // a0..a5

pub const MAX_ENTRIES: usize = 32;
const BUCKETS: usize = 64; // power of two, >= 2 x MAX_ENTRIES
pub const MAX_APPLETS: usize = 8;
pub const MAX_SLOTS: usize = 16;
/// Calls into guest providers nest at most this deep (the outermost call is depth 0).
pub const MAX_DEPTH: usize = 8;
/// Guest stack per nesting level, carved from the top of guest RAM.
pub const STACK_PER_LEVEL: u32 = 2048;
/// Only the kernel may register names in this namespace.
pub const KERNEL_NAMESPACE: &[u8] = b"kernel.";

/// What a native op gets: its arguments and access to the kernel (guest memory, console).
pub type NativeFn = fn(&mut Kernel, [u32; 6]) -> Result<[u32; 2], Error>;

#[derive(Clone, Copy)]
pub enum Provider {
    Native(NativeFn),
    Guest { applet: u8, entry: u32 },
}

/// Reserved for middleware: a range in a future hook pool. Always empty this round.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Hooks {
    pub first: u16,
    pub count: u16,
}

#[derive(Clone, Copy)]
pub struct Entry<'a> {
    pub hash: u64,
    /// The full name: a slice of the code blob, or `'static` for kernel ops.
    pub name: &'a [u8],
    pub provider: Provider,
    pub hooks: Hooks,
}

/// Who is registering: only the kernel may use [`KERNEL_NAMESPACE`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Owner {
    Kernel,
    Applet(u8),
}

/// Open-addressed hash index over an append-only entry array.
///
/// Entries never move, so an entry index stays valid for as long as the
/// registry lives. Growing past `MAX_ENTRIES` would mean a bigger entry array
/// plus rebuilding `buckets` (which only hold indices).
pub struct Registry<'a> {
    entries: [Option<Entry<'a>>; MAX_ENTRIES],
    len: usize,
    buckets: [u16; BUCKETS],
}

const EMPTY: u16 = u16::MAX;

impl Default for Registry<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Registry<'a> {
    pub const fn new() -> Self {
        Registry {
            entries: [None; MAX_ENTRIES],
            len: 0,
            buckets: [EMPTY; BUCKETS],
        }
    }

    /// The entry index for `hash`, whatever its name.
    pub fn lookup(&self, hash: u64) -> Option<u16> {
        let mut b = hash as usize & (BUCKETS - 1);
        loop {
            match self.buckets[b] {
                EMPTY => return None,
                i if self.entries[i as usize].map(|e| e.hash) == Some(hash) => return Some(i),
                _ => b = (b + 1) & (BUCKETS - 1),
            }
        }
    }

    /// The entry for an import: the hash must be present *and* carry the same name.
    pub fn resolve(&self, hash: u64, name: &[u8]) -> Result<u16, Error> {
        let i = self.lookup(hash).ok_or(Error::Unresolved(hash))?;
        if self.entries[i as usize].map(|e| e.name) != Some(name) {
            return Err(Error::NameMismatch(hash));
        }
        Ok(i)
    }

    /// The checks `register` makes, without changing anything.
    pub fn check(&self, hash: u64, name: &[u8], owner: Owner) -> Result<(), Error> {
        if owner != Owner::Kernel && name.starts_with(KERNEL_NAMESPACE) {
            return Err(Error::ReservedNamespace(hash));
        }
        match self.lookup(hash) {
            Some(i) if self.entries[i as usize].map(|e| e.name) != Some(name) => {
                Err(Error::HashCollision(hash))
            }
            Some(_) => Ok(()),
            None if self.len == MAX_ENTRIES => Err(Error::RegistryFull),
            None => Ok(()),
        }
    }

    /// Add `name`, or replace its provider (last registration wins; the entry
    /// index is unchanged, so existing bindings follow). The caller vouches
    /// that `hash` is the name's hash; `Kernel::load` checks that for applets.
    pub fn register(
        &mut self,
        hash: u64,
        name: &'a [u8],
        provider: Provider,
        owner: Owner,
    ) -> Result<u16, Error> {
        self.check(hash, name, owner)?;
        if let Some(i) = self.lookup(hash) {
            if let Some(e) = self.entries[i as usize].as_mut() {
                e.provider = provider;
            }
            return Ok(i);
        }
        let i = self.len as u16;
        self.entries[self.len] = Some(Entry {
            hash,
            name,
            provider,
            hooks: Hooks::default(),
        });
        self.len += 1;
        let mut b = hash as usize & (BUCKETS - 1);
        while self.buckets[b] != EMPTY {
            b = (b + 1) & (BUCKETS - 1);
        }
        self.buckets[b] = i;
        Ok(i)
    }

    pub fn entry(&self, i: u16) -> Option<&Entry<'a>> {
        self.entries.get(i as usize)?.as_ref()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// One loaded applet: its slot table maps import slot -> registry entry.
#[derive(Clone, Copy)]
pub struct Applet {
    pub slots: [u16; MAX_SLOTS],
    pub nslots: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Load failed: nobody provides this import (name hash).
    Unresolved(u64),
    /// An import's hash is registered, but under a different name.
    NameMismatch(u64),
    /// A manifest entry's hash isn't the FNV-1a 64 of its name.
    NameHashMismatch(u64),
    /// An applet tried to register a `kernel.*` name.
    ReservedNamespace(u64),
    /// A different name already has this hash (crafted or accidental collision).
    HashCollision(u64),
    RegistryFull,
    TooManyImports(u32),
    BadAppletId(u8),
    /// A call would nest deeper than MAX_DEPTH.
    DepthExceeded(usize),
    /// `ecall` with a slot the applet doesn't have.
    BadSlot(u32),
    /// Memory outside guest RAM or the code blob (op arguments, manifest names).
    BadPointer(u32),
    Fault(Fault),
}

impl From<Fault> for Error {
    fn from(f: Fault) -> Self {
        Error::Fault(f)
    }
}

/// Where kernel.uart.write.v1 bytes go.
pub trait Console {
    fn write(&mut self, bytes: &[u8]);
}

pub struct LoadStats {
    pub imports: u32,
    pub exports: u32,
}

pub struct Kernel<'a> {
    pub registry: Registry<'a>,
    pub applets: [Option<Applet>; MAX_APPLETS],
    /// All applet images, back to back (one read-only code region).
    pub blob: &'a [u8],
    pub blob_base: u32,
    /// Shared guest RAM (buffers and the per-level stacks).
    pub ram: &'a mut [u8],
    pub ram_base: u32,
    /// One decode table for the whole blob.
    pub table: &'a mut [fast::E],
    pub console: &'a mut dyn Console,
    pub now: fn() -> u32,
    /// Deepest nesting reached so far (for the report).
    pub max_depth_seen: usize,
    /// Guest instructions run (only with the `count` feature).
    #[cfg(feature = "count")]
    pub steps: u64,
}

fn rd32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(o..o.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn rd64(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        b.get(o..o.checked_add(8)?)?.try_into().ok()?,
    ))
}

fn op_uart_write(k: &mut Kernel, a: [u32; 6]) -> Result<[u32; 2], Error> {
    let (ptr, len) = (a[0], a[1]);
    k.guest_bytes(ptr, len).ok_or(Error::BadPointer(ptr))?;
    // Copy out in chunks: the console and guest memory both live in `k`.
    let mut buf = [0u8; 64];
    let mut done = 0u32;
    while done < len {
        let n = (len - done).min(64);
        buf[..n as usize].copy_from_slice(k.guest_bytes(ptr + done, n).unwrap());
        k.console.write(&buf[..n as usize]);
        done += n;
    }
    Ok([len, 0])
}

fn op_timer_now(k: &mut Kernel, _: [u32; 6]) -> Result<[u32; 2], Error> {
    Ok([(k.now)(), 0])
}

fn op_nop(_: &mut Kernel, _: [u32; 6]) -> Result<[u32; 2], Error> {
    Ok([0, 0])
}

/// Where the import/export tables of one applet are, and how big.
struct Tables {
    io: usize,
    n_imp: u32,
    eo: usize,
    n_exp: u32,
}

impl<'a> Kernel<'a> {
    /// A kernel with the three kernel ops registered.
    pub fn new(
        blob: &'a [u8],
        blob_base: u32,
        ram: &'a mut [u8],
        ram_base: u32,
        table: &'a mut [fast::E],
        console: &'a mut dyn Console,
        now: fn() -> u32,
    ) -> Result<Self, Error> {
        table.fill(fast::E::EMPTY);
        let mut k = Kernel {
            registry: Registry::new(),
            applets: [None; MAX_APPLETS],
            blob,
            blob_base,
            ram,
            ram_base,
            table,
            console,
            now,
            max_depth_seen: 0,
            #[cfg(feature = "count")]
            steps: 0,
        };
        let ops: [(&'static str, NativeFn); 3] = [
            ("kernel.uart.write.v1", op_uart_write),
            ("kernel.timer.now.v1", op_timer_now),
            ("kernel.nop.v1", op_nop),
        ];
        for (name, f) in ops {
            k.registry.register(
                fnv1a64(name),
                name.as_bytes(),
                Provider::Native(f),
                Owner::Kernel,
            )?;
        }
        Ok(k)
    }

    fn blob_off(&self, addr: u32) -> usize {
        addr.wrapping_sub(self.blob_base) as usize
    }

    /// Guest memory as host bytes: RAM or the code blob. Pointers pass straight through.
    pub fn guest_bytes(&self, ptr: u32, len: u32) -> Option<&[u8]> {
        let (p, l) = (ptr as usize, len as usize);
        let r = p.wrapping_sub(self.ram_base as usize);
        if r <= self.ram.len() && l <= self.ram.len() - r {
            return Some(&self.ram[r..r + l]);
        }
        let b = p.wrapping_sub(self.blob_base as usize);
        if b <= self.blob.len() && l <= self.blob.len() - b {
            return Some(&self.blob[b..b + l]);
        }
        None
    }

    /// A name inside the code blob (manifests point into their own image).
    fn blob_str(&self, addr: u32, len: u32) -> Result<&'a [u8], Error> {
        let blob: &'a [u8] = self.blob;
        let o = self.blob_off(addr);
        blob.get(o..o.wrapping_add(len as usize))
            .ok_or(Error::BadPointer(addr))
    }

    /// Import `i`: (hash, name). 16-byte entries `{hash: u64, name: u32, len: u32}`.
    fn import(&self, t: &Tables, i: u32) -> Result<(u64, &'a [u8]), Error> {
        let o = t.io + 8 + 16 * i as usize;
        let h = rd64(self.blob, o).ok_or(Error::BadPointer(o as u32))?;
        let (p, l) = (
            rd32(self.blob, o + 8).unwrap_or(0),
            rd32(self.blob, o + 12).unwrap_or(0),
        );
        Ok((h, self.blob_str(p, l)?))
    }

    /// Export `j`: (hash, function, name). 24-byte entries `{hash, func, name, len, _}`.
    fn export(&self, t: &Tables, j: u32) -> Result<(u64, u32, &'a [u8]), Error> {
        let o = t.eo + 8 + 24 * j as usize;
        let h = rd64(self.blob, o).ok_or(Error::BadPointer(o as u32))?;
        let f = rd32(self.blob, o + 8).ok_or(Error::BadPointer(o as u32))?;
        let (p, l) = (
            rd32(self.blob, o + 12).unwrap_or(0),
            rd32(self.blob, o + 16).unwrap_or(0),
        );
        Ok((h, f, self.blob_str(p, l)?))
    }

    /// Load applet `id` from its tables in the blob (`count: u32, _: u32`, then entries).
    ///
    /// Everything is checked before anything changes, so a failed load leaves
    /// the registry as it was:
    /// - every hash must be the FNV-1a 64 of its name (`NameHashMismatch`);
    /// - exports may not use `kernel.*` (`ReservedNamespace`), nor reuse a hash
    ///   held by a different name (`HashCollision`);
    /// - every import must resolve to an entry *with the same name*, from the
    ///   registry or the applet's own exports (`Unresolved`, `NameMismatch`).
    ///
    /// Then exports are registered and imports bound to entry indices.
    pub fn load(&mut self, id: u8, imports: u32, exports: Option<u32>) -> Result<LoadStats, Error> {
        if id as usize >= MAX_APPLETS {
            return Err(Error::BadAppletId(id));
        }
        let io = self.blob_off(imports);
        let n_imp = rd32(self.blob, io).ok_or(Error::BadPointer(imports))?;
        if n_imp as usize > MAX_SLOTS {
            return Err(Error::TooManyImports(n_imp));
        }
        let (eo, n_exp) = match exports {
            Some(e) => {
                let eo = self.blob_off(e);
                (eo, rd32(self.blob, eo).ok_or(Error::BadPointer(e))?)
            }
            None => (0, 0),
        };
        let t = Tables {
            io,
            n_imp,
            eo,
            n_exp,
        };

        // 1. Check everything.
        for j in 0..t.n_exp {
            let (h, _, name) = self.export(&t, j)?;
            if fnv1a64_bytes(name) != h {
                return Err(Error::NameHashMismatch(h));
            }
            self.registry.check(h, name, Owner::Applet(id))?;
        }
        for i in 0..t.n_imp {
            let (h, name) = self.import(&t, i)?;
            if fnv1a64_bytes(name) != h {
                return Err(Error::NameHashMismatch(h));
            }
            let mut own = false;
            for j in 0..t.n_exp {
                let (eh, _, en) = self.export(&t, j)?;
                own |= eh == h && en == name;
            }
            if !own {
                self.registry.resolve(h, name)?;
            }
        }
        // 2. Register exports (last registration wins).
        for j in 0..t.n_exp {
            let (h, f, name) = self.export(&t, j)?;
            self.registry.register(
                h,
                name,
                Provider::Guest {
                    applet: id,
                    entry: f,
                },
                Owner::Applet(id),
            )?;
        }
        // 3. Bind imports: slot -> entry index. Calls never hash or compare names.
        let mut a = Applet {
            slots: [u16::MAX; MAX_SLOTS],
            nslots: t.n_imp as u8,
        };
        for i in 0..t.n_imp {
            let (h, name) = self.import(&t, i)?;
            a.slots[i as usize] = self.registry.resolve(h, name)?;
        }
        self.applets[id as usize] = Some(a);
        Ok(LoadStats {
            imports: t.n_imp,
            exports: t.n_exp,
        })
    }

    /// Run guest code of applet `applet` at `entry`, servicing its mesh ops.
    pub fn call(&mut self, applet: u8, entry: u32, args: &[u32]) -> Result<u32, Error> {
        self.run_guest(applet, entry, args, 0)
    }

    fn run_guest(
        &mut self,
        applet: u8,
        entry: u32,
        args: &[u32],
        depth: usize,
    ) -> Result<u32, Error> {
        if depth >= MAX_DEPTH {
            return Err(Error::DepthExceeded(depth));
        }
        self.max_depth_seen = self.max_depth_seen.max(depth);
        let slots = self
            .applets
            .get(applet as usize)
            .copied()
            .flatten()
            .ok_or(Error::BadAppletId(applet))?;
        let top = self.ram_base.wrapping_add(self.ram.len() as u32);
        let sp = top.wrapping_sub(depth as u32 * STACK_PER_LEVEL);
        let mut cpu = Cpu::new();
        {
            let mem = Mem {
                image: self.blob,
                image_base: self.blob_base,
                ram: self.ram,
                ram_base: self.ram_base,
            };
            cpu.start(&mem, self.table.len(), entry, args, sp)?;
        }
        loop {
            let stop = {
                let mut mem = Mem {
                    image: self.blob,
                    image_base: self.blob_base,
                    ram: self.ram,
                    ram_base: self.ram_base,
                };
                cpu.run(&mut mem, self.table)?
            };
            match stop {
                Stop::Returned(v) => {
                    #[cfg(feature = "count")]
                    {
                        self.steps += cpu.steps;
                    }
                    return Ok(v);
                }
                Stop::Ecall => {
                    let slot = cpu.x[REG_SLOT];
                    if slot >= slots.nslots as u32 {
                        return Err(Error::BadSlot(slot));
                    }
                    let idx = slots.slots[slot as usize];
                    let mut a = [0u32; 6];
                    a.copy_from_slice(&cpu.x[REG_ARG0..REG_ARG0 + 6]);
                    let r = self.dispatch(idx, a, depth)?;
                    cpu.x[10] = r[0];
                    cpu.x[11] = r[1];
                }
            }
        }
    }

    /// Run the provider of registry entry `idx`. Callers can't tell native from guest.
    pub fn dispatch(&mut self, idx: u16, a: [u32; 6], depth: usize) -> Result<[u32; 2], Error> {
        let e = *self.registry.entry(idx).ok_or(Error::BadSlot(idx as u32))?;
        match e.provider {
            Provider::Native(f) => f(self, a),
            Provider::Guest { applet, entry } => {
                Ok([self.run_guest(applet, entry, &a, depth + 1)?, 0])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nop(_: &mut Kernel, _: [u32; 6]) -> Result<[u32; 2], Error> {
        Ok([0, 0])
    }

    #[test]
    fn collision_with_a_different_name_is_rejected() {
        // A real FNV collision is hard to build in a test, so fake one: two names, one hash.
        let mut r = Registry::new();
        let h = 0x1234_5678_9abc_def0;
        r.register(h, b"test.echo.v1", Provider::Native(nop), Owner::Applet(1))
            .unwrap();
        assert_eq!(
            r.register(h, b"evil.echo.v1", Provider::Native(nop), Owner::Applet(2))
                .err(),
            Some(Error::HashCollision(h))
        );
        // Same name again: allowed (last registration wins), same entry index.
        assert_eq!(
            r.register(h, b"test.echo.v1", Provider::Native(nop), Owner::Applet(2))
                .ok(),
            Some(0)
        );
        // An import with the right hash but the wrong name doesn't resolve.
        assert_eq!(r.resolve(h, b"evil.echo.v1"), Err(Error::NameMismatch(h)));
        assert_eq!(r.resolve(h, b"test.echo.v1"), Ok(0));
    }

    #[test]
    fn only_the_kernel_registers_kernel_names() {
        let mut r = Registry::new();
        let h = fnv1a64("kernel.nop.v1");
        assert_eq!(
            r.register(h, b"kernel.nop.v1", Provider::Native(nop), Owner::Applet(3))
                .err(),
            Some(Error::ReservedNamespace(h))
        );
        assert!(r
            .register(h, b"kernel.nop.v1", Provider::Native(nop), Owner::Kernel)
            .is_ok());
    }
}
