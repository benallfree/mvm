//! Split relocatable RISC-V ELF objects into self-contained, hashable chunks.
//!
//! A chunk is one allocated section (or one string of a mergeable string pool).
//! Its relocated fields are blanked and replaced by slots. A slot names its
//! target either by symbol name (named chunks: functions, statics) or by the
//! target's content hash (anonymous chunks: literals, jump tables, string pieces).

use crate::stable::stable_name;
use object::elf;
use object::read::elf::{ElfFile32, FileHeader, SectionHeader};
use object::{
    Endianness, Object, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget,
    SectionIndex, SymbolKind, SymbolSection,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};

pub type Hash = [u8; 32];
pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Text = 0,
    ROData = 1,
    Data = 2,
    Bss = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SlotKind {
    Hi20,
    Lo12I,
    Lo12S,
    PcrelHi20,
    PcrelLo12I,
    PcrelLo12S,
    Call,
    Jal,
    Branch,
    RvcJump,
    RvcBranch,
    Abs32,
}

#[derive(Clone, Debug)]
pub enum Target {
    /// A chunk in the store. Hashed as its name if it has one, else its content hash.
    Chunk(usize),
    /// Undefined in its object; resolved by name in `Store::resolve`.
    Extern(String),
    /// PC-relative low part: the offset (in this chunk) of its paired PCREL_HI20 slot.
    PcrelHi(u32),
}

#[derive(Clone, Debug)]
pub struct Slot {
    pub at: u32,
    pub kind: SlotKind,
    pub target: Target,
    /// Offset into the target chunk (or the raw addend, for an extern).
    pub addend: i64,
}

#[derive(Clone, Debug)]
pub struct Chunk {
    /// Non-local-label symbols at offset 0. Empty means anonymous.
    pub names: Vec<String>,
    pub kind: Kind,
    pub align: u64,
    pub bytes: Vec<u8>,
    pub slots: Vec<Slot>,
    pub origin: String,
}

#[derive(Default, Debug)]
pub struct Report {
    pub relocs: BTreeMap<String, usize>,
    pub pcrel_pairs: usize,
    pub string_pieces: usize,
    pub skipped_sections: Vec<String>,
    /// crate -> (v0 names, plain names) it defines.
    pub names: BTreeMap<String, (usize, usize)>,
}

#[derive(Default)]
pub struct Store {
    pub chunks: Vec<Chunk>,
    pub by_name: BTreeMap<String, usize>,
    pub report: Report,
    pub missing: Vec<String>,
    /// stable name -> the raw symbol it came from.
    pub raw_of: BTreeMap<String, String>,
    hashes: Vec<Option<Hash>>,
}

/// How a section was cut into chunks: (start, end, chunk index).
type Pieces = Vec<(u64, u64, usize)>;

fn locate(
    pieces: &HashMap<SectionIndex, Pieces>,
    si: SectionIndex,
    off: i64,
) -> Result<(usize, i64)> {
    let ps = pieces
        .get(&si)
        .ok_or_else(|| format!("reference into unchunked section {}", si.0))?;
    for &(s, e, ci) in ps {
        if off >= s as i64 && off < e as i64 {
            return Ok((ci, off - s as i64));
        }
    }
    // One-past-the-end of the last piece (e.g. a slice end pointer).
    if let Some(&(s, e, ci)) = ps.last()
        && off == e as i64
    {
        return Ok((ci, off - s as i64));
    }
    Err(format!("offset {off} outside section {}", si.0))
}

fn rd32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn wr32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn rd16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
fn wr16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// Clear the bits a slot of this kind will write.
fn blank(b: &mut [u8], at: usize, kind: SlotKind) {
    let m32 = |b: &mut [u8], at: usize, keep: u32| wr32(b, at, rd32(b, at) & keep);
    match kind {
        SlotKind::Hi20 | SlotKind::PcrelHi20 | SlotKind::Jal => m32(b, at, 0x0000_0fff),
        SlotKind::Lo12I | SlotKind::PcrelLo12I => m32(b, at, 0x000f_ffff),
        SlotKind::Lo12S | SlotKind::PcrelLo12S | SlotKind::Branch => m32(b, at, 0x01ff_f07f),
        SlotKind::Call => {
            m32(b, at, 0x0000_0fff);
            m32(b, at + 4, 0x000f_ffff);
        }
        SlotKind::RvcJump => wr16(b, at, rd16(b, at) & 0xe003),
        SlotKind::RvcBranch => wr16(b, at, rd16(b, at) & 0xe383),
        SlotKind::Abs32 => wr32(b, at, 0),
    }
}

pub fn slot_width(kind: SlotKind) -> usize {
    match kind {
        SlotKind::Call => 8,
        SlotKind::RvcJump | SlotKind::RvcBranch => 2,
        _ => 4,
    }
}

fn is_label(name: &str) -> bool {
    name.is_empty() || name.starts_with(".L") || name.starts_with('$')
}

/// Expand an `.rlib`/`.a` into its member objects; pass `.o` through.
pub fn load_input(path: &str) -> Result<Vec<(String, Vec<u8>)>> {
    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if path.ends_with(".rlib") || path.ends_with(".a") {
        let ar = object::read::archive::ArchiveFile::parse(&*data)
            .map_err(|e| format!("{path}: {e}"))?;
        let mut out = Vec::new();
        for m in ar.members() {
            let m = m.map_err(|e| e.to_string())?;
            let name = String::from_utf8_lossy(m.name()).to_string();
            if name.ends_with(".o") {
                out.push((name, m.data(&*data).map_err(|e| e.to_string())?.to_vec()));
            }
        }
        Ok(out)
    } else {
        let name = path.rsplit('/').next().unwrap().to_string();
        Ok(vec![(name, data)])
    }
}

impl Store {
    pub fn add_object(&mut self, label: &str, data: &[u8]) -> Result<()> {
        let file = ElfFile32::<Endianness>::parse(data).map_err(|e| format!("{label}: {e}"))?;
        let endian = file.endian();
        if file.elf_header().e_machine(endian) != elf::EM_RISCV {
            return Err(format!("{label}: not a RISC-V object"));
        }
        let mut pieces: HashMap<SectionIndex, Pieces> = HashMap::new();

        // 1. Cut every allocated section into chunks.
        for sec in file.sections() {
            let hdr = sec.elf_section_header();
            let flags = hdr.sh_flags(endian);
            let name = sec.name().unwrap_or("?").to_string();
            if flags & elf::SHF_ALLOC == 0 {
                if sec.size() > 0 && !name.starts_with(".rela") {
                    self.report.skipped_sections.push(format!("{label}:{name}"));
                }
                continue;
            }
            if sec.size() == 0 {
                continue;
            }
            if flags & elf::SHF_TLS != 0 {
                return Err(format!("{label}:{name}: TLS section not handled"));
            }
            let kind = if flags & elf::SHF_EXECINSTR != 0 {
                Kind::Text
            } else if hdr.sh_type(endian) == elf::SHT_NOBITS {
                Kind::Bss
            } else if flags & elf::SHF_WRITE != 0 {
                Kind::Data
            } else {
                Kind::ROData
            };
            let bytes = if kind == Kind::Bss {
                vec![0; sec.size() as usize]
            } else {
                sec.data().map_err(|e| e.to_string())?.to_vec()
            };
            let mut ps = Pieces::new();
            if flags & elf::SHF_MERGE != 0 && flags & elf::SHF_STRINGS != 0 {
                if hdr.sh_entsize(endian) != 1 {
                    return Err(format!("{label}:{name}: wide string pool not handled"));
                }
                // Split the pool at each terminating zero byte.
                let mut start = 0;
                for (i, &b) in bytes.iter().enumerate() {
                    if b == 0 {
                        ps.push((
                            start as u64,
                            i as u64 + 1,
                            self.push(Kind::ROData, 1, &bytes[start..=i], label, &name, start),
                        ));
                        self.report.string_pieces += 1;
                        start = i + 1;
                    }
                }
                if start != bytes.len() {
                    return Err(format!("{label}:{name}: unterminated string in pool"));
                }
            } else {
                ps.push((
                    0,
                    bytes.len() as u64,
                    self.push(kind, sec.align(), &bytes, label, &name, 0),
                ));
            }
            pieces.insert(sec.index(), ps);
        }

        // 2. Attach symbol names. A named symbol must start its chunk.
        for sym in file.symbols() {
            if matches!(sym.kind(), SymbolKind::Section | SymbolKind::File) {
                continue;
            }
            let raw = sym.name().map_err(|e| e.to_string())?;
            if is_label(raw) {
                continue;
            }
            if let SymbolSection::Section(si) = sym.section() {
                if !pieces.contains_key(&si) {
                    continue;
                }
                let (ci, off) = locate(&pieces, si, sym.address() as i64)?;
                let name = &self.intern(label, raw)?;
                if off != 0 {
                    return Err(format!("{label}: symbol {name} at +{off} inside a chunk"));
                }
                if self.by_name.insert(name.to_string(), ci).is_some() {
                    return Err(format!("{label}: duplicate symbol {raw}"));
                }
                self.chunks[ci].names.push(name.to_string());
                let krate = label.split(['-', '.']).next().unwrap_or(label).to_string();
                let e = self.report.names.entry(krate).or_default();
                if raw.starts_with("_R") {
                    e.0 += 1
                } else {
                    e.1 += 1
                }
            }
        }

        // 3. Turn relocations into slots, blanking the fields they cover.
        for sec in file.sections() {
            if !pieces.contains_key(&sec.index()) {
                if sec.relocations().next().is_some() {
                    self.report
                        .skipped_sections
                        .push(format!("{label}:rela for {}", sec.name().unwrap_or("?")));
                }
                continue;
            }
            for (off, rel) in sec.relocations() {
                let RelocationFlags::Elf { r_type } = rel.flags() else {
                    unreachable!()
                };
                let kind = match r_type {
                    elf::R_RISCV_32 => SlotKind::Abs32,
                    elf::R_RISCV_HI20 => SlotKind::Hi20,
                    elf::R_RISCV_LO12_I => SlotKind::Lo12I,
                    elf::R_RISCV_LO12_S => SlotKind::Lo12S,
                    elf::R_RISCV_PCREL_HI20 => SlotKind::PcrelHi20,
                    elf::R_RISCV_PCREL_LO12_I => SlotKind::PcrelLo12I,
                    elf::R_RISCV_PCREL_LO12_S => SlotKind::PcrelLo12S,
                    elf::R_RISCV_CALL | elf::R_RISCV_CALL_PLT => SlotKind::Call,
                    elf::R_RISCV_JAL => SlotKind::Jal,
                    elf::R_RISCV_BRANCH => SlotKind::Branch,
                    elf::R_RISCV_RVC_JUMP => SlotKind::RvcJump,
                    elf::R_RISCV_RVC_BRANCH => SlotKind::RvcBranch,
                    elf::R_RISCV_RELAX => {
                        return Err(format!("{label}: R_RISCV_RELAX present; build with -relax"));
                    }
                    other => {
                        return Err(format!(
                            "{label}: unknown relocation type {other} at {off:#x}"
                        ));
                    }
                };
                *self.report.relocs.entry(format!("{kind:?}")).or_default() += 1;
                let (ci, at) = locate(&pieces, sec.index(), off as i64)?;
                let RelocationTarget::Symbol(sidx) = rel.target() else {
                    return Err(format!("{label}: relocation without a symbol"));
                };
                let sym = file.symbol_by_index(sidx).map_err(|e| e.to_string())?;
                let addend = rel.addend();
                let (target, addend) = match kind {
                    SlotKind::PcrelLo12I | SlotKind::PcrelLo12S => {
                        // The symbol is the label on the auipc, not the real target.
                        let SymbolSection::Section(si) = sym.section() else {
                            return Err(format!("{label}: pcrel_lo against undefined label"));
                        };
                        let (hci, hat) = locate(&pieces, si, sym.address() as i64 + addend)?;
                        if hci != ci {
                            return Err(format!(
                                "{label}: pcrel_lo and its hi20 in different chunks"
                            ));
                        }
                        self.report.pcrel_pairs += 1;
                        (Target::PcrelHi(hat as u32), 0)
                    }
                    _ => match sym.section() {
                        SymbolSection::Section(si) => {
                            let (tci, toff) = locate(&pieces, si, sym.address() as i64 + addend)?;
                            (Target::Chunk(tci), toff)
                        }
                        SymbolSection::Undefined => (
                            Target::Extern(
                                self.intern(label, sym.name().map_err(|e| e.to_string())?)?,
                            ),
                            addend,
                        ),
                        other => return Err(format!("{label}: symbol in {other:?} not handled")),
                    },
                };
                let c = &mut self.chunks[ci];
                if at as usize + slot_width(kind) > c.bytes.len() {
                    return Err(format!("{label}: slot runs past its chunk"));
                }
                blank(&mut c.bytes, at as usize, kind);
                c.slots.push(Slot {
                    at: at as u32,
                    kind,
                    target,
                    addend,
                });
            }
        }

        // 4. Every pcrel_lo must find its hi20 in the same chunk.
        for c in &mut self.chunks {
            c.slots.sort_by_key(|s| s.at);
            for s in &c.slots {
                if let Target::PcrelHi(h) = s.target
                    && !c
                        .slots
                        .iter()
                        .any(|t| t.at == h && t.kind == SlotKind::PcrelHi20)
                {
                    return Err(format!(
                        "{}: pcrel_lo at {:#x} has no hi20 at {h:#x}",
                        c.origin, s.at
                    ));
                }
            }
        }
        Ok(())
    }

    /// Map a raw symbol to its stable name. Two raw names with one stable name is fatal.
    fn intern(&mut self, label: &str, raw: &str) -> Result<String> {
        let st = stable_name(raw)?;
        match self.raw_of.get(&st) {
            Some(prev) if prev != raw => {
                return Err(format!(
                    "{label}: stable name collision: {st}\n  {prev}\n  {raw}"
                ));
            }
            Some(_) => {}
            None => {
                self.raw_of.insert(st.clone(), raw.to_string());
            }
        }
        Ok(st)
    }

    fn push(
        &mut self,
        kind: Kind,
        align: u64,
        bytes: &[u8],
        label: &str,
        sec: &str,
        off: usize,
    ) -> usize {
        let origin = if off == 0 {
            format!("{label}:{sec}")
        } else {
            format!("{label}:{sec}+{off:#x}")
        };
        self.chunks.push(Chunk {
            names: vec![],
            kind,
            align,
            bytes: bytes.to_vec(),
            slots: vec![],
            origin,
        });
        self.chunks.len() - 1
    }

    /// Bind externs to the chunks that define them. Returns the names nobody defines.
    pub fn resolve(&mut self) -> Vec<String> {
        let mut missing = Vec::new();
        for c in &mut self.chunks {
            for s in &mut c.slots {
                if let Target::Extern(n) = &s.target {
                    match self.by_name.get(n) {
                        Some(&ci) => s.target = Target::Chunk(ci),
                        None => missing.push(n.clone()),
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        self.hashes = vec![None; self.chunks.len()];
        missing
    }

    pub fn hash(&mut self, i: usize) -> Result<Hash> {
        self.hash_inner(i, &mut vec![])
    }

    fn hash_inner(&mut self, i: usize, stack: &mut Vec<usize>) -> Result<Hash> {
        if let Some(h) = self.hashes[i] {
            return Ok(h);
        }
        if stack.contains(&i) {
            return Err(format!(
                "cycle among anonymous chunks at {}",
                self.chunks[i].origin
            ));
        }
        stack.push(i);
        let c = self.chunks[i].clone();
        let mut h = Sha256::new();
        h.update(b"mvm-chunk-v1");
        h.update([c.kind as u8]);
        h.update((c.align as u32).to_le_bytes());
        h.update((c.bytes.len() as u32).to_le_bytes());
        h.update(&c.bytes);
        h.update((c.slots.len() as u32).to_le_bytes());
        for s in &c.slots {
            h.update(s.at.to_le_bytes());
            h.update([s.kind as u8]);
            match &s.target {
                Target::PcrelHi(at) => {
                    h.update([2]);
                    h.update(at.to_le_bytes());
                }
                Target::Chunk(j) => match self.chunks[*j].names.first() {
                    Some(n) => {
                        h.update([0]);
                        h.update((n.len() as u32).to_le_bytes());
                        h.update(n.as_bytes());
                    }
                    None => {
                        h.update([1]);
                        h.update(self.hash_inner(*j, stack)?);
                    }
                },
                // Unresolved: hashed by name like any named target; only linking needs it defined.
                Target::Extern(n) => {
                    h.update([0]);
                    h.update((n.len() as u32).to_le_bytes());
                    h.update(n.as_bytes());
                }
            }
            h.update(s.addend.to_le_bytes());
        }
        stack.pop();
        let out: Hash = h.finalize().into();
        self.hashes[i] = Some(out);
        Ok(out)
    }

    /// The first chunk with the same hash as `i`. Call after hashing every chunk.
    pub fn canonical(&self, i: usize) -> usize {
        // Writable statics are never merged: two zero-initialised counters
        // have the same content but must stay two variables.
        if matches!(self.chunks[i].kind, Kind::Data | Kind::Bss) {
            return i;
        }
        let h = self.hashes[i];
        self.hashes.iter().position(|x| *x == h).unwrap()
    }

    /// Distinct chunks reachable from `root` (including itself), in discovery order.
    pub fn closure(&self, root: usize) -> Vec<usize> {
        self.closure_many(&[root])
    }

    /// Distinct chunks reachable from any of `roots`, in discovery order.
    pub fn closure_many(&self, roots: &[usize]) -> Vec<usize> {
        let mut seen = vec![false; self.chunks.len()];
        let mut order = Vec::new();
        for &r in roots {
            let r = self.canonical(r);
            if !seen[r] {
                seen[r] = true;
                order.push(r);
            }
        }
        let mut i = 0;
        while i < order.len() {
            for s in &self.chunks[order[i]].slots {
                if let Target::Chunk(j) = s.target {
                    let j = self.canonical(j);
                    if !seen[j] {
                        seen[j] = true;
                        order.push(j);
                    }
                }
            }
            i += 1;
        }
        order
    }
}

pub fn hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn display_name(c: &Chunk) -> String {
    match c.names.first() {
        Some(n) => n.clone(),
        None => "(anon)".to_string(),
    }
}
