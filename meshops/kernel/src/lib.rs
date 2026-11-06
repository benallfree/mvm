#![no_std]
//! The MVM kernel's mesh-op layer (rounds 7 and 7b). See MESHOPS.md and MEMMODEL.md.
//!
//! - [`Registry`]: name hash -> entry; entries keep the full name (the hash is
//!   only an index), and never move, so imports bind to entry indices.
//! - [`Kernel::load`]: takes an applet's [`Manifest`] (first-class: static
//!   size/alignment, imports and exports with full names), checks it all
//!   (including that it fits) before changing anything, places its static
//!   block in RAM, and binds its imports.
//! - [`Kernel::call`]: runs guest code on the one kernel-owned stack; each
//!   `ecall` looks up the slot (no hashing) and runs the provider, native or
//!   guest. Guest providers run one nesting level deeper, on the caller's
//!   stack, with their own `gp`.
//!
//! Memory model (round 7b): guest pointers are real addresses (on 32-bit
//! chips; the 64-bit x86 test host uses stand-in addresses), code is never
//! patched, each applet's statics live in a RAM block reached through `gp`.
//! Round 7c: two RAM segments per applet, the gp window (at most 4 KB) and a
//! bulk segment of any size reached through bulk slots in the window. See
//! BULK.md.

pub mod drivers;
pub mod exec;
pub mod io;

pub use exec::{ExecStats, TState, TicketSlot, DEFAULT_BUDGET, MAX_TICKETS, NO_TASK};
use interp::fast::{self, Cpu, Stop};
use interp::{Fault, Mem, Region};
use mvm_abi::{call, names, BULK_BIAS, BULK_PAGE, GP_WINDOW};
pub use mvm_abi::{bulk_slots, fnv1a64, fnv1a64_bytes, GP_BIAS, KERNEL_NAMESPACE};

pub const MAX_ENTRIES: usize = 32;
const BUCKETS: usize = 64; // power of two, >= 2 x MAX_ENTRIES
pub const MAX_APPLETS: usize = 16;
/// Distinct flash images loaded (instances of one image share a region).
pub const MAX_REGIONS: usize = 12;
pub const MAX_SLOTS: usize = 16;
/// Calls into guest providers nest at most this deep (the outermost call is depth 0).
pub const MAX_DEPTH: usize = 8;

/// What a native op gets: its arguments and access to the kernel (guest memory, console).
pub type NativeFn = fn(&mut Kernel, [u32; 6]) -> Result<[u32; 2], Error>;

#[derive(Clone, Copy)]
pub enum Provider {
    Native(NativeFn),
    /// Applet `applet`'s code at absolute address `entry`.
    Guest {
        applet: u8,
        entry: u32,
    },
}

/// Reserved for middleware: a range in a future hook pool. Always empty so far.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Hooks {
    pub first: u16,
    pub count: u16,
}

#[derive(Clone, Copy)]
pub struct Entry<'a> {
    pub hash: u64,
    /// The full name (from a manifest, or `'static` for kernel ops).
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

// ---- Manifests (first-class; produced by the build, see build_applets.py) ----

#[derive(Clone, Copy)]
pub struct Import<'a> {
    pub hash: u64,
    pub name: &'a str,
}

#[derive(Clone, Copy)]
pub struct Export<'a> {
    pub hash: u64,
    pub name: &'a str,
    /// Offset of the function in the applet's flash image.
    pub entry: u32,
}

/// Where something lives: the flash image, the gp window or the bulk segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seg {
    Flash,
    Gp,
    Bulk,
}

/// A pointer stored in the applet's statics, fixed up in RAM at load.
#[derive(Clone, Copy)]
pub struct DataReloc {
    /// The segment holding the 32-bit word (`Gp` or `Bulk`) and its offset there.
    pub seg: Seg,
    pub at: u32,
    /// The segment the pointer targets, and the offset in it.
    pub to: Seg,
    pub off: u32,
}

/// A RAM segment: size, alignment and initial bytes (a prefix; the rest is zeroed).
#[derive(Clone, Copy)]
pub struct Segment<'a> {
    pub size: u32,
    pub align: u32,
    pub init: &'a [u8],
}

/// Everything the kernel needs to know about an applet, without running it.
#[derive(Clone, Copy)]
pub struct Manifest<'a> {
    /// The flash image (code + rodata), position independent.
    pub image: &'a [u8],
    /// Small statics, reached `gp`-relative (at most 4 KB, bulk slots first).
    pub gp: Segment<'a>,
    /// Large statics, reached through the bulk slots (any size).
    pub bulk: Segment<'a>,
    /// Number of bulk slots at the start of the gp window.
    pub bulk_slots: u32,
    /// Round 7d: offsets of the `init()` and `poll(task)` exports of an async
    /// applet (`None` for a plain one).
    pub init: Option<u32>,
    pub poll: Option<u32>,
    pub data_relocs: &'a [DataReloc],
    pub imports: &'a [Import<'a>],
    pub exports: &'a [Export<'a>],
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

/// One loaded applet.
#[derive(Clone, Copy)]
pub struct Applet {
    /// Its flash image's code region.
    pub region: u8,
    /// Its gp window (guest address, length) and its `gp`.
    pub block: u32,
    pub block_len: u32,
    pub gp: u32,
    /// Its bulk segment (guest address, length; 0 if none).
    pub bulk: u32,
    pub bulk_len: u32,
    /// Import slot -> registry entry.
    pub slots: [u16; MAX_SLOTS],
    pub nslots: u8,
    /// Round 7d: `poll` export offset (`u32::MAX`: not an async applet).
    pub poll: u32,
    /// Tasks spawned and not finished; tasks to poll in the next batch (bit = task id).
    pub spawned: u8,
    pub ready: u8,
    /// Stopped by the kernel (budget or fault): never polled or called again.
    pub faulted: bool,
}

/// A flash image loaded once (shared by every instance of that applet).
struct CodeRegion<'a> {
    base: u32,
    bytes: &'a [u8],
    table: &'a mut [fast::E],
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
    /// The applet's statics (`need` bytes with alignment padding) don't fit
    /// in the `free` RAM left below the stack. Checked before anything changes.
    NoRoom {
        need: u32,
        free: u32,
    },
    /// No decode-table space (or region slot) left for another flash image.
    NoCodeRoom,
    RegistryFull,
    TooManyImports(u32),
    BadAppletId(u8),
    BadManifest,
    /// A call would nest deeper than MAX_DEPTH.
    DepthExceeded(usize),
    /// `ecall` with a slot the applet doesn't have.
    BadSlot(u32),
    /// Memory outside guest RAM or any loaded flash image.
    BadPointer(u32),
    /// Round 7d: an `init`/`poll` ran out of its instruction budget.
    Budget,
    /// `poll` returned something other than Pending or Done.
    BadPollResult(u32),
    /// The applet was stopped by the kernel earlier.
    Faulted(u8),
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

/// Guest addresses are real addresses on 32-bit chips. A 64-bit host can't
/// hand out 32-bit pointers, so it uses stand-ins (the only host-specific
/// line; the interpreter and registry don't change).
const REAL_ADDRESSES: bool = usize::BITS == 32;
const HOST_RAM_BASE: u32 = 0x2000_0000;
const HOST_FLASH_BASE: u32 = 0x0100_0000;

fn addr_of(bytes: &[u8], stand_in: u32) -> u32 {
    if REAL_ADDRESSES {
        bytes.as_ptr() as usize as u32
    } else {
        stand_in
    }
}

pub struct Kernel<'a> {
    pub registry: Registry<'a>,
    pub applets: [Option<Applet>; MAX_APPLETS],
    regions: [Option<CodeRegion<'a>>; MAX_REGIONS],
    /// Read-only views of every region, for loads that leave the current one.
    views: [Region<'a>; MAX_REGIONS],
    nviews: usize,
    /// Decode tables not yet handed to a region.
    table_free: &'a mut [fast::E],
    /// The RAM arena: applet static blocks from the bottom, the one shared
    /// stack (`stack_size` bytes) at the top.
    ram: &'a mut [u8],
    pub ram_base: u32,
    ram_used: u32,
    pub stack_size: u32,
    /// One register file per nesting level, reused call after call.
    cpus: [Cpu; MAX_DEPTH],
    pub console: &'a mut dyn Console,
    pub now: fn() -> u32,
    /// Deepest nesting reached so far (for the report).
    pub max_depth_seen: usize,
    /// Guest instructions run (only with the `count` feature).
    #[cfg(feature = "count")]
    pub steps: u64,
    /// Round 7d: the rings shared with the drivers.
    pub io: &'a io::Io,
    pub(crate) x: exec::ExecState,
    /// Budget for each `init`/`poll` call, in control transfers.
    pub budget: u32,
}

fn op_uart_write(k: &mut Kernel, a: [u32; 6]) -> Result<[u32; 2], Error> {
    let (ptr, len) = (a[0], a[1]);
    // One lookup, then write straight from guest memory (split borrow: the
    // console and guest memory are both fields of `k`).
    let bytes = guest_bytes_in(&k.ram[..], k.ram_base, &k.views[..k.nviews], ptr, len)
        .ok_or(Error::BadPointer(ptr))?;
    k.console.write(bytes);
    Ok([len, 0])
}

/// `ptr..ptr+len` in RAM or one of the read-only region views.
fn guest_bytes_in<'b>(
    ram: &'b [u8],
    ram_base: u32,
    views: &'b [Region<'_>],
    ptr: u32,
    len: u32,
) -> Option<&'b [u8]> {
    let (p, l) = (ptr as usize, len as usize);
    let r = p.wrapping_sub(ram_base as usize);
    if r <= ram.len() && l <= ram.len() - r {
        return Some(&ram[r..r + l]);
    }
    views.iter().find_map(|v| {
        let o = p.wrapping_sub(v.base as usize);
        (o <= v.bytes.len() && l <= v.bytes.len() - o).then(|| &v.bytes[o..o + l])
    })
}

fn op_timer_now(k: &mut Kernel, _: [u32; 6]) -> Result<[u32; 2], Error> {
    Ok([(k.now)(), 0])
}

fn op_nop(_: &mut Kernel, _: [u32; 6]) -> Result<[u32; 2], Error> {
    Ok([0, 0])
}

fn op_mem_owner(k: &mut Kernel, a: [u32; 6]) -> Result<[u32; 2], Error> {
    let owner = k.owner_of(a[0]).map(|id| id as u32 + 1).unwrap_or(0);
    Ok([owner, 0])
}

/// FNV-1a 32 of `len` bytes at a guest pointer, read by the kernel.
fn op_mem_sum(k: &mut Kernel, a: [u32; 6]) -> Result<[u32; 2], Error> {
    let b = k.guest_bytes(a[0], a[1]).ok_or(Error::BadPointer(a[0]))?;
    let mut s = 0x811c_9dc5u32;
    for &x in b {
        s = (s ^ x as u32).wrapping_mul(0x0100_0193);
    }
    Ok([s, 0])
}

impl<'a> Kernel<'a> {
    /// A kernel owning `ram` (statics + the shared stack of `stack_size`
    /// bytes at the top) and `tables` (decode tables for loaded images).
    pub fn new(
        ram: &'a mut [u8],
        stack_size: u32,
        tables: &'a mut [fast::E],
        console: &'a mut dyn Console,
        now: fn() -> u32,
        io: &'a io::Io,
    ) -> Result<Self, Error> {
        let ram_base = addr_of(ram, HOST_RAM_BASE);
        let mut k = Kernel {
            registry: Registry::new(),
            applets: [None; MAX_APPLETS],
            regions: [const { None }; MAX_REGIONS],
            views: [Region {
                base: 0,
                bytes: &[],
            }; MAX_REGIONS],
            nviews: 0,
            table_free: tables,
            ram,
            ram_base,
            ram_used: 0,
            stack_size,
            cpus: [const { Cpu::new() }; MAX_DEPTH],
            console,
            now,
            max_depth_seen: 0,
            #[cfg(feature = "count")]
            steps: 0,
            io,
            x: exec::ExecState::new(),
            budget: DEFAULT_BUDGET,
        };
        let ops: [(&'static str, NativeFn); 12] = [
            (names::UART_WRITE, op_uart_write),
            (names::TIMER_NOW, op_timer_now),
            (names::NOP, op_nop),
            (names::MEM_OWNER, op_mem_owner),
            (names::MEM_SUM, op_mem_sum),
            (names::TIMER_START, exec::op_timer_start),
            (names::TICKET_CHECK, exec::op_ticket_check),
            (names::TICKET_CANCEL, exec::op_ticket_cancel),
            (names::TASK_SPAWN, exec::op_task_spawn),
            (names::RADIO_SEND, exec::op_radio_send),
            (names::RADIO_RECV, exec::op_radio_recv),
            (names::GPIO_WAIT, exec::op_gpio_wait),
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

    /// True when guest addresses are the chip's real addresses.
    pub fn real_addresses(&self) -> bool {
        REAL_ADDRESSES
    }

    /// Bytes of RAM still free for static blocks (the stack is reserved).
    pub fn free_ram(&self) -> u32 {
        self.ram.len() as u32 - self.stack_size - self.ram_used
    }

    /// The shared stack (for measuring peak use).
    pub fn stack(&mut self) -> &mut [u8] {
        let n = self.ram.len();
        &mut self.ram[n - self.stack_size as usize..]
    }

    /// Guest memory as host bytes: RAM or a loaded flash image. Pointers pass straight through.
    pub fn guest_bytes(&self, ptr: u32, len: u32) -> Option<&[u8]> {
        guest_bytes_in(self.ram, self.ram_base, &self.views[..self.nviews], ptr, len)
    }

    /// Writable guest RAM at `ptr..ptr+len` (never flash).
    pub fn guest_bytes_mut(&mut self, ptr: u32, len: u32) -> Option<&mut [u8]> {
        let (p, l) = (ptr as usize, len as usize);
        let r = p.wrapping_sub(self.ram_base as usize);
        (r <= self.ram.len() && l <= self.ram.len() - r).then(|| &mut self.ram[r..r + l])
    }

    /// The applet whose gp window or bulk segment holds `ptr`.
    pub fn owner_of(&self, ptr: u32) -> Option<u8> {
        self.applets.iter().enumerate().find_map(|(i, a)| {
            let a = (*a)?;
            (ptr.wrapping_sub(a.block) < a.block_len || ptr.wrapping_sub(a.bulk) < a.bulk_len)
                .then_some(i as u8)
        })
    }

    /// The code region (and its address) for `image`, if already loaded.
    fn region_of(&self, image: &[u8]) -> Option<u8> {
        self.regions
            .iter()
            .position(|r| {
                r.as_ref().is_some_and(|r| {
                    r.bytes.as_ptr() == image.as_ptr() && r.bytes.len() == image.len()
                })
            })
            .map(|i| i as u8)
    }

    /// The address of offset `off` in applet `id`'s flash image.
    pub fn code_addr(&self, id: u8, off: u32) -> Option<u32> {
        let a = (*self.applets.get(id as usize)?)?;
        Some(
            self.regions[a.region as usize]
                .as_ref()?
                .base
                .wrapping_add(off),
        )
    }

    /// Load applet `id` from its manifest.
    ///
    /// Everything is checked before anything changes, so a failed load
    /// leaves the kernel as it was:
    /// - the statics fit in free RAM (`NoRoom`), and a new image fits in the
    ///   decode-table space (`NoCodeRoom`);
    /// - every hash is the FNV-1a 64 of its name (`NameHashMismatch`);
    /// - exports don't use `kernel.*` (`ReservedNamespace`) or another name's
    ///   hash (`HashCollision`);
    /// - every import resolves, by hash *and* name (`Unresolved`, `NameMismatch`).
    ///
    /// Then: the image gets a code region (shared if already loaded), the
    /// static block is placed, initialised and relocated, `gp` is set,
    /// exports are registered and imports bound.
    pub fn load(&mut self, id: u8, m: &Manifest<'a>) -> Result<(), Error> {
        if id as usize >= MAX_APPLETS {
            return Err(Error::BadAppletId(id));
        }
        if m.imports.len() > MAX_SLOTS {
            return Err(Error::TooManyImports(m.imports.len() as u32));
        }
        let seg_ok = |g: &Segment| g.init.len() as u32 <= g.size && g.align.is_power_of_two();
        if !seg_ok(&m.gp)
            || !seg_ok(&m.bulk)
            || m.gp.size > GP_WINDOW
            || m.bulk_slots != bulk_slots(m.bulk.size)
            || m.gp.size < 4 * m.bulk_slots
            || !m.image.len().is_multiple_of(2)
        {
            return Err(Error::BadManifest);
        }
        // 1. Fit: the gp window, then the bulk segment, both from free RAM.
        // (u64: a huge manifest size must fail the check, not wrap.)
        let base = self.ram_base as u64;
        let up = |v: u64, a: u32| v.div_ceil(a as u64) * a as u64;
        let gp_at = up(base + self.ram_used as u64, m.gp.align);
        let bulk_at = up(gp_at + m.gp.size as u64, m.bulk.align);
        let end = bulk_at + m.bulk.size as u64;
        let need = (end - base - self.ram_used as u64).min(u32::MAX as u64) as u32;
        let free = self.free_ram();
        if need > free {
            return Err(Error::NoRoom { need, free });
        }
        let (start, bulk_start) = ((gp_at - base) as u32, (bulk_at - base) as u32);
        let region = self.region_of(m.image);
        if region.is_none()
            && (self.table_free.len() < m.image.len() / 2
                || self.regions.iter().all(|r| r.is_some()))
        {
            return Err(Error::NoCodeRoom);
        }
        // 2. Names.
        for e in m.exports {
            if fnv1a64(e.name) != e.hash {
                return Err(Error::NameHashMismatch(e.hash));
            }
            self.registry
                .check(e.hash, e.name.as_bytes(), Owner::Applet(id))?;
        }
        for i in m.imports {
            if fnv1a64(i.name) != i.hash {
                return Err(Error::NameHashMismatch(i.hash));
            }
            let own = m
                .exports
                .iter()
                .any(|e| e.hash == i.hash && e.name == i.name);
            if !own {
                self.registry.resolve(i.hash, i.name.as_bytes())?;
            }
        }
        // 3. Code region (instances of one image share it, and its decode table).
        let r = match region {
            Some(r) => r,
            None => {
                let slot = self
                    .regions
                    .iter()
                    .position(|r| r.is_none())
                    .ok_or(Error::NoCodeRoom)?;
                let tables = core::mem::take(&mut self.table_free);
                let (table, rest) = tables.split_at_mut(m.image.len() / 2);
                self.table_free = rest;
                table.fill(fast::E::EMPTY);
                let base = addr_of(m.image, HOST_FLASH_BASE + 0x0010_0000 * slot as u32);
                self.regions[slot] = Some(CodeRegion {
                    base,
                    bytes: m.image,
                    table,
                });
                self.views[self.nviews] = Region {
                    base,
                    bytes: m.image,
                };
                self.nviews += 1;
                slot as u8
            }
        };
        let code_base = self.regions[r as usize]
            .as_ref()
            .map(|c| c.base)
            .unwrap_or(0);
        // 4. Segments: initialise, zero the rest, write the bulk slots, apply
        // pointer fix-ups.
        let block = self.ram_base + start;
        let bulk = self.ram_base + bulk_start;
        for (at, g) in [(start, &m.gp), (bulk_start, &m.bulk)] {
            let seg = &mut self.ram[at as usize..(at + g.size) as usize];
            seg.fill(0);
            seg[..g.init.len()].copy_from_slice(g.init);
        }
        let mut put = |seg_at: u32, seg_len: u32, at: u32, v: u32| -> Result<(), Error> {
            if at.checked_add(4).is_none_or(|e| e > seg_len) {
                return Err(Error::BadManifest);
            }
            let a = (seg_at + at) as usize;
            self.ram[a..a + 4].copy_from_slice(&v.to_le_bytes());
            Ok(())
        };
        for k in 0..m.bulk_slots {
            put(start, m.gp.size, 4 * k, bulk + BULK_PAGE * k + BULK_BIAS)?;
        }
        for d in m.data_relocs {
            let v = match d.to {
                Seg::Flash => code_base,
                Seg::Gp => block,
                Seg::Bulk => bulk,
            } + d.off;
            match d.seg {
                Seg::Gp => put(start, m.gp.size, d.at, v)?,
                Seg::Bulk => put(bulk_start, m.bulk.size, d.at, v)?,
                Seg::Flash => return Err(Error::BadManifest),
            }
        }
        self.ram_used = bulk_start + m.bulk.size;
        // 5. Exports, then imports (an applet may import what it exports).
        for e in m.exports {
            let p = Provider::Guest {
                applet: id,
                entry: code_base + e.entry,
            };
            self.registry
                .register(e.hash, e.name.as_bytes(), p, Owner::Applet(id))?;
        }
        let mut a = Applet {
            region: r,
            block,
            block_len: m.gp.size,
            gp: block + GP_BIAS,
            bulk,
            bulk_len: m.bulk.size,
            slots: [u16::MAX; MAX_SLOTS],
            nslots: m.imports.len() as u8,
            poll: m.poll.unwrap_or(u32::MAX),
            spawned: 0,
            ready: 0,
            faulted: false,
        };
        for (k, i) in m.imports.iter().enumerate() {
            a.slots[k] = self.registry.resolve(i.hash, i.name.as_bytes())?;
        }
        self.applets[id as usize] = Some(a);
        // Round 7d: an async applet's init() runs once, under the budget. It
        // creates its tasks (kernel.task.spawn.v1) and returns.
        if let Some(init) = m.init {
            let budget = self.budget;
            if let Err(e) = self.call_budgeted(id, init, &[], budget) {
                self.fault(id);
                return Err(e);
            }
        }
        Ok(())
    }

    /// Run applet `applet`'s code at offset `entry` of its image, servicing its mesh ops.
    /// Host calls are not budgeted (the round-7 cost loops run for millions
    /// of instructions); `init` and `poll` are (see [`Kernel::call_budgeted`]).
    pub fn call(&mut self, applet: u8, entry: u32, args: &[u32]) -> Result<u32, Error> {
        self.call_budgeted(applet, entry, args, u32::MAX)
    }

    fn call_inner(&mut self, applet: u8, entry: u32, args: &[u32]) -> Result<u32, Error> {
        let at = self
            .code_addr(applet, entry)
            .ok_or(Error::BadAppletId(applet))?;
        let top = self.ram_base + self.ram.len() as u32;
        self.run_guest(applet, at, args, 0, top)
    }

    /// Run `f` with the interpreter's view of memory for code region `r`,
    /// that region's decode table, and the register file for `depth`.
    fn with_cpu<R>(
        &mut self,
        r: usize,
        depth: usize,
        f: impl FnOnce(&mut Cpu, &mut Mem, &mut [fast::E]) -> R,
    ) -> Result<R, Error> {
        let Kernel {
            regions,
            views,
            nviews,
            ram,
            ram_base,
            cpus,
            ..
        } = self;
        let reg = regions
            .get_mut(r)
            .and_then(|r| r.as_mut())
            .ok_or(Error::BadPointer(0))?;
        let mut mem = Mem {
            image: reg.bytes,
            image_base: reg.base,
            ram,
            ram_base: *ram_base,
            others: &views[..*nviews],
        };
        Ok(f(&mut cpus[depth], &mut mem, &mut *reg.table))
    }

    /// The code region holding address `t`.
    fn region_at(&self, t: u32) -> Option<usize> {
        self.regions.iter().position(|r| {
            r.as_ref()
                .is_some_and(|r| t.wrapping_sub(r.base) < r.bytes.len() as u32)
        })
    }

    fn run_guest(
        &mut self,
        applet: u8,
        entry: u32,
        args: &[u32],
        depth: usize,
        sp: u32,
    ) -> Result<u32, Error> {
        if depth >= MAX_DEPTH {
            return Err(Error::DepthExceeded(depth));
        }
        self.max_depth_seen = self.max_depth_seen.max(depth);
        let a = self
            .applets
            .get(applet as usize)
            .copied()
            .flatten()
            .ok_or(Error::BadAppletId(applet))?;
        if a.faulted {
            return Err(Error::Faulted(applet));
        }
        self.x.stats.guest_calls += 1;
        if depth == 0 {
            self.io.guest_running.store(true, core::sync::atomic::Ordering::Release);
        }
        let r = self.run_guest_inner(applet, a, entry, args, depth, sp);
        if depth == 0 {
            self.io.guest_running.store(false, core::sync::atomic::Ordering::Release);
        }
        r
    }

    fn run_guest_inner(
        &mut self,
        applet: u8,
        a: Applet,
        entry: u32,
        args: &[u32],
        depth: usize,
        sp: u32,
    ) -> Result<u32, Error> {
        let mut r = a.region as usize;
        self.with_cpu(r, depth, |cpu, mem, table| {
            cpu.start(mem, table.len(), entry, args, sp, a.gp)
        })??;
        loop {
            let budget = self.x.budget_left;
            let stop = self.with_cpu(r, depth, |cpu, mem, table| {
                cpu.budget = budget;
                let s = cpu.run(mem, table);
                (s, cpu.budget)
            })?;
            self.x.budget_left = stop.1;
            match stop.0? {
                Stop::Budget => return Err(Error::Budget),
                Stop::Returned(v) => {
                    #[cfg(feature = "count")]
                    {
                        self.steps += core::mem::replace(&mut self.cpus[depth].steps, 0);
                    }
                    return Ok(v);
                }
                Stop::Jump(t) => {
                    // Code in another region: library code runs with the caller's gp.
                    r = self.region_at(t).ok_or(Error::BadPointer(t))?;
                    self.with_cpu(r, depth, |cpu, mem, table| {
                        cpu.resume_at(mem, table.len(), t)
                    })??;
                }
                Stop::Ecall => {
                    let x = &self.cpus[depth].x;
                    let slot = x[call::REG_SLOT];
                    if slot >= a.nslots as u32 {
                        return Err(Error::BadSlot(slot));
                    }
                    let mut args = [0u32; call::NUM_ARGS];
                    args.copy_from_slice(&x[call::REG_ARG0..call::REG_ARG0 + call::NUM_ARGS]);
                    // A guest provider continues on this caller's stack.
                    let caller_sp = x[2] & !15;
                    // Native ops see who called (ticket ownership).
                    self.x.caller = applet;
                    let res = self.dispatch(a.slots[slot as usize], args, depth, caller_sp)?;
                    let x = &mut self.cpus[depth].x;
                    x[call::REG_RET0] = res[0];
                    x[call::REG_RET1] = res[1];
                }
            }
        }
    }

    /// Run the provider of registry entry `idx`. Callers can't tell native from guest.
    fn dispatch(
        &mut self,
        idx: u16,
        a: [u32; 6],
        depth: usize,
        sp: u32,
    ) -> Result<[u32; 2], Error> {
        let e = *self.registry.entry(idx).ok_or(Error::BadSlot(idx as u32))?;
        match e.provider {
            Provider::Native(f) => f(self, a),
            Provider::Guest { applet, entry } => {
                Ok([self.run_guest(applet, entry, &a, depth + 1, sp)?, 0])
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
