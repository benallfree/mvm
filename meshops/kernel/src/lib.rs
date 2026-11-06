#![no_std]
//! The MVM kernel's mesh-op layer (round seven). See MESHOPS.md.
//!
//! - [`Registry`]: name hash -> entry. Entries never move once added, so a
//!   resolved import can hold an entry index (a "live binding").
//! - [`Kernel::load`]: reads an applet's import/export tables from its image,
//!   registers its exports, and resolves its imports into a slot table.
//! - [`Kernel::call`]: runs guest code; each `ecall` looks up the slot (no hashing)
//!   and runs the provider, native or guest. Guest providers re-enter the
//!   interpreter with a fresh register file, one level deeper.

use interp::fast::{self, Cpu, Stop};
use interp::{Fault, Mem};

/// FNV-1a 64 over the full versioned name, e.g. `kernel.uart.write.v1`.
pub const fn fnv1a64(s: &str) -> u64 {
    let b = s.as_bytes();
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
pub struct Entry {
    pub hash: u64,
    pub provider: Provider,
    pub hooks: Hooks,
}

/// Open-addressed hash index over an append-only entry array.
///
/// Entries never move, so an entry index stays valid for as long as the
/// registry lives. Growing past `MAX_ENTRIES` would mean a bigger entry array
/// plus rebuilding `buckets` (which only hold indices).
pub struct Registry {
    entries: [Option<Entry>; MAX_ENTRIES],
    len: usize,
    buckets: [u16; BUCKETS],
}

const EMPTY: u16 = u16::MAX;

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub const fn new() -> Self {
        Registry {
            entries: [None; MAX_ENTRIES],
            len: 0,
            buckets: [EMPTY; BUCKETS],
        }
    }

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

    /// Add `hash`, or replace its provider (last registration wins; the
    /// entry index is unchanged, so existing bindings follow).
    pub fn register(&mut self, hash: u64, provider: Provider) -> Result<u16, Error> {
        if let Some(i) = self.lookup(hash) {
            self.entries[i as usize].as_mut().unwrap().provider = provider;
            return Ok(i);
        }
        if self.len == MAX_ENTRIES {
            return Err(Error::RegistryFull);
        }
        let i = self.len as u16;
        self.entries[self.len] = Some(Entry {
            hash,
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

    pub fn entry(&self, i: u16) -> Option<&Entry> {
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
    RegistryFull,
    TooManyImports(u32),
    BadAppletId(u8),
    /// A call would nest deeper than MAX_DEPTH.
    DepthExceeded(usize),
    /// `ecall` with a slot the applet doesn't have.
    BadSlot(u32),
    /// A native op was handed memory outside guest RAM or the code blob.
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
    pub registry: Registry,
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
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn rd64(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
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
        k.registry
            .register(UART_WRITE, Provider::Native(op_uart_write))?;
        k.registry
            .register(TIMER_NOW, Provider::Native(op_timer_now))?;
        k.registry.register(NOP, Provider::Native(op_nop))?;
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

    /// Load applet `id` from its tables in the blob. All imports are checked
    /// first (against the registry and the applet's own exports), so a failed
    /// load changes nothing. Then exports are registered and imports bound.
    pub fn load(&mut self, id: u8, imports: u32, exports: Option<u32>) -> Result<LoadStats, Error> {
        if id as usize >= MAX_APPLETS {
            return Err(Error::BadAppletId(id));
        }
        let blob = self.blob;
        let io = self.blob_off(imports);
        let n_imp = rd32(blob, io).ok_or(Error::BadPointer(imports))?;
        if n_imp as usize > MAX_SLOTS {
            return Err(Error::TooManyImports(n_imp));
        }
        let (eo, n_exp) = match exports {
            Some(e) => {
                let eo = self.blob_off(e);
                (eo, rd32(blob, eo).ok_or(Error::BadPointer(e))?)
            }
            None => (0, 0),
        };
        let exp = |j: u32| -> Option<(u64, u32)> {
            let o = eo + 8 + 16 * j as usize;
            Some((rd64(blob, o)?, rd32(blob, o + 8)?))
        };
        // 1. Check every import resolves (registry or our own exports).
        for i in 0..n_imp {
            let h = rd64(blob, io + 8 + 8 * i as usize).ok_or(Error::BadPointer(imports))?;
            let own = (0..n_exp).any(|j| exp(j).map(|e| e.0) == Some(h));
            if !own && self.registry.lookup(h).is_none() {
                return Err(Error::Unresolved(h));
            }
        }
        // 2. Register exports (last registration wins).
        for j in 0..n_exp {
            let (h, f) = exp(j).ok_or(Error::BadPointer(exports.unwrap_or(0)))?;
            self.registry.register(
                h,
                Provider::Guest {
                    applet: id,
                    entry: f,
                },
            )?;
        }
        // 3. Bind imports: slot -> entry index. Calls never hash.
        let mut a = Applet {
            slots: [u16::MAX; MAX_SLOTS],
            nslots: n_imp as u8,
        };
        for i in 0..n_imp as usize {
            let h = rd64(blob, io + 8 + 8 * i).unwrap();
            a.slots[i] = self.registry.lookup(h).ok_or(Error::Unresolved(h))?;
        }
        self.applets[id as usize] = Some(a);
        Ok(LoadStats {
            imports: n_imp,
            exports: n_exp,
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
