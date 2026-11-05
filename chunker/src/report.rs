//! Round-two measurements: name stability/collisions, manifest size, LTO overlap.

use crate::chunk::{Hash, Kind, Store, Target, hex};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};

fn name_hash8(name: &str) -> [u8; 8] {
    Sha256::digest(name.as_bytes())[..8].try_into().unwrap()
}

/// Per-crate name counts, plus collision checks for the short encodings.
pub fn names(store: &mut Store) -> Result<(), String> {
    println!("{:<20} {:>8} {:>8}", "crate", "v0", "plain");
    let (mut v0, mut plain) = (0, 0);
    for (k, (a, b)) in &store.report.names {
        println!("{k:<20} {a:>8} {b:>8}");
        v0 += a;
        plain += b;
    }
    println!("{:<20} {v0:>8} {plain:>8}", "total");
    println!("stable-name collisions: 0 (a collision is a hard error while loading)");

    let mut seen: HashMap<[u8; 8], &str> = HashMap::new();
    let mut coll = Vec::new();
    for st in store.raw_of.keys() {
        if let Some(prev) = seen.insert(name_hash8(st), st) {
            coll.push(format!("{prev} / {st}"));
        }
    }
    println!(
        "8-byte name-hash collisions: {} over {} names",
        coll.len(),
        store.raw_of.len()
    );

    let mut full = BTreeSet::new();
    for i in 0..store.chunks.len() {
        full.insert(store.hash(i)?);
    }
    let short: BTreeSet<[u8; 16]> = full.iter().map(|h| h[..16].try_into().unwrap()).collect();
    println!(
        "16-byte chunk-hash collisions: {} over {} distinct chunks",
        full.len() - short.len(),
        full.len()
    );
    if coll.is_empty() && full.len() == short.len() {
        Ok(())
    } else {
        Err(format!("collisions: {coll:?}"))
    }
}

pub struct Closure {
    /// Distinct (canonical) chunks reachable from the entries.
    pub chunks: Vec<usize>,
    /// The manifest: every name referenced by a reachable slot, plus the entries.
    pub names: BTreeMap<String, Hash>,
    pub unresolved: BTreeSet<String>,
}

pub fn closure(store: &mut Store, entries: &[String]) -> Result<Closure, String> {
    let mut chunks = BTreeSet::new();
    let mut names = BTreeMap::new();
    let mut unresolved = BTreeSet::new();
    for e in entries {
        let ci = *store.by_name.get(e).ok_or(format!("no entry {e}"))?;
        names.insert(e.clone(), store.hash(ci)?);
        chunks.extend(store.closure(ci));
    }
    for &ci in &chunks.clone() {
        for s in store.chunks[ci].slots.clone() {
            match &s.target {
                Target::Chunk(j) => {
                    if let Some(n) = store.chunks[*j].names.first().cloned() {
                        names.insert(n, store.hash(*j)?);
                    }
                }
                Target::Extern(n) => {
                    unresolved.insert(n.clone());
                }
                Target::PcrelHi(_) => {}
            }
        }
    }
    Ok(Closure {
        chunks: chunks.into_iter().collect(),
        names,
        unresolved,
    })
}

fn varint_len(mut v: usize) -> usize {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

pub fn manifest(store: &mut Store, label: &str, entries: &[String]) -> Result<(), String> {
    let c = closure(store, entries)?;
    if !c.unresolved.is_empty() {
        return Err(format!("{label}: unresolved {:?}", c.unresolved));
    }
    let mut by_kind: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut reach = 0;
    let mut slots = 0;
    for &ci in &c.chunks {
        let ch = &store.chunks[ci];
        let e = by_kind.entry(format!("{:?}", ch.kind)).or_default();
        e.0 += 1;
        e.1 += ch.bytes.len();
        reach += ch.bytes.len();
        slots += ch.slots.len();
    }
    let n = c.names.len();
    let name_bytes: usize = c.names.keys().map(|k| varint_len(k.len()) + k.len()).sum();
    let header = varint_len(n);
    let a = header + name_bytes + 32 * n;
    let b = header + n * (8 + 16);
    let dev = header
        + c.names
            .keys()
            .map(|k| 8 + varint_len(k.len()) + k.len())
            .sum::<usize>();
    let pct = |x: usize| 100.0 * x as f64 / reach as f64;
    let kinds: Vec<String> = by_kind
        .iter()
        .map(|(k, (cnt, b))| format!("{k} {cnt} chunks/{b} B"))
        .collect();
    println!("== {label}: entries {entries:?}");
    println!(
        "reachable: {} chunks, {reach} bytes ({}), {slots} slots",
        c.chunks.len(),
        kinds.join(", ")
    );
    let longest = c.names.keys().map(|k| k.len()).max().unwrap_or(0);
    println!(
        "manifest: {n} named entries, names {name_bytes} B total (mean {:.1}, longest {longest})",
        name_bytes as f64 / n as f64
    );
    println!("| encoding | shipped manifest | % of reachable | dev file |");
    println!("|---|---|---|---|");
    println!(
        "| a) stable names + 32 B hashes | {a} B | {:.1}% | - |",
        pct(a)
    );
    println!(
        "| b) 8 B name hash + 16 B chunk hash | {b} B | {:.1}% | - |",
        pct(b)
    );
    println!(
        "| c) b + names in a dev file | {b} B | {:.1}% | {dev} B |",
        pct(b)
    );
    if std::env::var_os("MANIFEST_VERBOSE").is_some() {
        for (k, h) in &c.names {
            println!("  {} {k}", &hex(h)[..16]);
        }
    }
    Ok(())
}

/// Compare the applet closure between two builds (e.g. lto=off vs lto=fat).
pub fn compare(a: &mut Store, b: &mut Store, entries: &[String]) -> Result<(), String> {
    let ca = closure(a, entries)?;
    let cb = closure(b, entries)?;
    let size = |s: &Store, c: &Closure, kind: Option<Kind>| -> usize {
        c.chunks
            .iter()
            .map(|&i| &s.chunks[i])
            .filter(|ch| kind.is_none_or(|k| ch.kind == k))
            .map(|ch| ch.bytes.len())
            .sum()
    };
    let ha: BTreeSet<Hash> = ca
        .chunks
        .iter()
        .map(|&i| a.hash(i))
        .collect::<Result<_, _>>()?;
    let hb: BTreeSet<Hash> = cb
        .chunks
        .iter()
        .map(|&i| b.hash(i))
        .collect::<Result<_, _>>()?;
    let shared = ha.intersection(&hb).count();
    if std::env::var_os("COMPARE_VERBOSE").is_some() {
        for &i in &ca.chunks {
            if hb.contains(&a.hash(i)?) {
                let ch = &a.chunks[i];
                let name = ch.names.first().cloned().unwrap_or_else(|| "(anon)".into());
                println!(
                    "  shared: {:?} {} B {name}  [{}]",
                    ch.kind,
                    ch.bytes.len(),
                    ch.origin.split(':').next().unwrap_or("")
                );
            }
        }
    }
    let (sa, sb) = (size(a, &ca, None), size(b, &cb, None));
    println!("| build | chunks | reachable bytes | of which text |");
    println!("|---|---|---|---|");
    println!(
        "| A | {} | {sa} | {} |",
        ca.chunks.len(),
        size(a, &ca, Some(Kind::Text))
    );
    println!(
        "| B | {} | {sb} | {} |",
        cb.chunks.len(),
        size(b, &cb, Some(Kind::Text))
    );
    println!(
        "size B vs A: {:+} bytes ({:+.1}%)",
        sb as i64 - sa as i64,
        100.0 * (sb as f64 / sa as f64 - 1.0)
    );
    println!(
        "shared chunk hashes: {shared} = {:.1}% of A's {}, {:.1}% of B's {}",
        100.0 * shared as f64 / ha.len() as f64,
        ha.len(),
        100.0 * shared as f64 / hb.len() as f64,
        hb.len()
    );
    Ok(())
}
