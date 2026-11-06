mod chunk;
mod emu;
mod link;
mod report;
mod rv;
mod stable;

use chunk::{Kind, Store, Target, display_name, hex};

const USAGE: &str = "usage:
  chunker dump      <objects/rlibs...>   every chunk, its slots and hash, plus a relocation report
  chunker hashes    <objects/rlibs...>   one line per named chunk and the anonymous chunks it uses (for diffing)
  chunker roundtrip <objects/rlibs...>   link spike_entry/spike_entry2, run them in the emulator, compare to native
  chunker names     <objects/rlibs...>   stable-name counts per crate; 8-byte name / 16-byte chunk hash collisions
  chunker manifest  --entry E [--entry E2] [--label L] <objects/rlibs...>   manifest size in three encodings
  chunker link      --entry E [--entry E2] -o OUT <objects/rlibs...>   flat image at 0x1000 (bench only): OUT.bin + OUT.rs
  chunker link-pic  [--bulk-threshold N] --entry E [--entry E2] -o OUT <objects/rlibs...>   position-independent: OUT.bin (flash), OUT.static (gp window init), OUT.bulk (bulk init), OUT.json
  chunker compare   --entry E <build A inputs...> -- <build B inputs...>    size and shared hashes, A vs B";

fn load(paths: &[String]) -> Result<Store, String> {
    let mut store = Store::default();
    for p in paths {
        for (name, data) in chunk::load_input(p)? {
            store.add_object(&name, &data)?;
        }
    }
    store.missing = store.resolve();
    for i in 0..store.chunks.len() {
        store.hash(i)?;
    }
    Ok(store)
}

fn preview(c: &chunk::Chunk) -> String {
    if c.kind == Kind::Text {
        return String::new();
    }
    let s: String = c
        .bytes
        .iter()
        .take(24)
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '.'
            }
        })
        .collect();
    format!(" {s:?}")
}

fn target_str(store: &mut Store, t: &Target) -> String {
    match t {
        Target::Chunk(j) => {
            let c = &store.chunks[*j];
            match c.names.first() {
                Some(_) => display_name(c),
                None => format!("anon:{}", &hex(&store.hash(*j).unwrap())[..12]),
            }
        }
        Target::PcrelHi(h) => format!("<pcrel_hi at {h:#x}>"),
        Target::Extern(n) => format!("extern {n}"),
    }
}

fn dump(store: &mut Store) {
    let r = &store.report;
    println!("relocations: {:?}", r.relocs);
    println!(
        "pcrel pairs: {}, string pieces: {}",
        r.pcrel_pairs, r.string_pieces
    );
    println!("skipped (not allocated): {:?}", r.skipped_sections);
    println!("unresolved: {:?}", store.missing);
    let mut uniq = std::collections::BTreeSet::new();
    for i in 0..store.chunks.len() {
        let h = store.hash(i).unwrap();
        uniq.insert(h);
        let c = store.chunks[i].clone();
        println!(
            "\n{} {:?} len={} align={} {}{}\n    from {}",
            &hex(&h)[..12],
            c.kind,
            c.bytes.len(),
            c.align,
            display_name(&c),
            preview(&c),
            c.origin
        );
        for s in &c.slots {
            println!(
                "    {:#06x} {:<11} -> {} {:+}",
                s.at,
                format!("{:?}", s.kind),
                target_str(store, &s.target),
                s.addend
            );
        }
    }
    println!(
        "\n{} chunks, {} distinct hashes",
        store.chunks.len(),
        uniq.len()
    );
}

fn hashes(store: &mut Store) {
    let mut lines = Vec::new();
    for i in 0..store.chunks.len() {
        let c = store.chunks[i].clone();
        if c.names.is_empty() {
            continue;
        }
        let mut line = format!("{} {}", hex(&store.hash(i).unwrap()), display_name(&c));
        let mut anon: Vec<String> = c
            .slots
            .iter()
            .filter_map(|s| match s.target {
                Target::Chunk(j) if store.chunks[j].names.is_empty() => {
                    let t = store.chunks[j].clone();
                    Some(format!(
                        "\n    uses {} {:?}{}",
                        &hex(&store.hash(j).unwrap())[..16],
                        t.kind,
                        preview(&t)
                    ))
                }
                _ => None,
            })
            .collect();
        anon.sort();
        anon.dedup();
        line.extend(anon);
        lines.push(line);
    }
    lines.sort_by(|a, b| {
        a.split_once(' ')
            .unwrap()
            .1
            .cmp(b.split_once(' ').unwrap().1)
    });
    for l in lines {
        println!("{l}");
    }
}

fn roundtrip(store: &Store) -> Result<(), String> {
    let natives: [(&str, extern "C" fn(u32) -> u32); 2] = [
        ("spike_entry", testcrate::spike_entry),
        ("spike_entry2", testcrate::spike_entry2),
    ];
    let mut ok = true;
    for (name, native) in natives {
        let entry = *store.by_name.get(name).ok_or(format!("no {name}"))?;
        let base = 0x1000;
        let img = link::link(store, entry, base)?;
        println!(
            "{name}: linked {} chunks into {} bytes",
            img.addr.len(),
            img.bytes.len()
        );
        for arg in [0u32, 1, 7, 1234, 0xdead_beef] {
            let mut cpu = emu::Cpu::new(1 << 20);
            cpu.mem[base as usize..base as usize + img.bytes.len()].copy_from_slice(&img.bytes);
            let got = cpu.call(img.addr[&entry], &[arg], 1_000_000)?;
            let want = native(arg);
            let mark = if got == want { "ok" } else { "MISMATCH" };
            ok &= got == want;
            println!(
                "  {name}({arg:#x}) = {got:#010x}, native {want:#010x}  {mark}  ({} steps)",
                cpu.steps
            );
        }
    }
    if ok {
        Ok(())
    } else {
        Err("round trip mismatch".into())
    }
}

/// Run applet_entry in the emulator against the native build, and check that an
/// out-of-range index ends in the panic handler.
fn applet_roundtrip(store: &Store) -> Result<(), String> {
    let entry = *store.by_name.get("applet_entry").ok_or("no applet_entry")?;
    let base = 0x1000u32;
    let img = link::link(store, entry, base)?;
    println!(
        "applet_entry: linked {} chunks into {} bytes",
        img.addr.len(),
        img.bytes.len()
    );
    let src: Vec<u8> = (0..40u8)
        .map(|i| i.wrapping_mul(37).wrapping_add(11))
        .collect();
    let (src_at, out_at, out_len) = (0x8_0000u32, 0x9_0000u32, 64usize);
    let mut ok = true;
    for idx in [0usize, 3, 39, 40, 1000] {
        let mut cpu = emu::Cpu::new(1 << 20);
        cpu.mem[base as usize..][..img.bytes.len()].copy_from_slice(&img.bytes);
        cpu.mem[src_at as usize..][..src.len()].copy_from_slice(&src);
        let args = [out_at, out_len as u32, src_at, src.len() as u32, idx as u32];
        let got = cpu.call(img.addr[&entry], &args, 200_000);
        if idx < src.len() {
            let mut out = vec![0u8; out_len];
            let want = unsafe {
                applet::applet_entry(out.as_mut_ptr(), out_len, src.as_ptr(), src.len(), idx)
            };
            let got = got?;
            let emu_out = &cpu.mem[out_at as usize..][..out_len];
            let same = got == want && emu_out == &out[..];
            ok &= same;
            let text = String::from_utf8_lossy(
                &out[..out.iter().position(|&b| b < 32).unwrap_or(out_len)],
            )
            .to_string();
            println!(
                "  idx {idx:>4}: {} ret {got} vs native {want}, {:?}  ({} steps)",
                if same { "ok" } else { "MISMATCH" },
                text,
                cpu.steps
            );
        } else {
            // The panic handler is `loop {}`, so a panic shows up as the step limit with pc inside it.
            let handler = *store
                .by_name
                .get("__rustc::rust_begin_unwind")
                .ok_or("no panic handler")?;
            let h = img.addr[&store.canonical(handler)];
            let len = store.chunks[handler].bytes.len() as u32;
            let in_handler = got.is_err() && (h..h + len).contains(&cpu.pc);
            ok &= in_handler;
            println!(
                "  idx {idx:>4}: {} ({:?}, pc {:#x}, handler at {h:#x})",
                if in_handler {
                    "ok, panicked into rust_begin_unwind"
                } else {
                    "NO PANIC"
                },
                got,
                cpu.pc
            );
        }
    }
    if ok {
        Ok(())
    } else {
        Err("applet round trip failed".into())
    }
}

/// Link the entries into one flat image at `base`. Writes `<out>.bin` and
/// `<out>.rs` (base, entry addresses, and an include of the image).
fn link_out(store: &Store, entries: &[String], out: &str) -> Result<(), String> {
    let base = 0x1000u32;
    let roots: Vec<usize> = entries
        .iter()
        .map(|e| store.by_name.get(e).copied().ok_or(format!("no entry {e}")))
        .collect::<Result<_, _>>()?;
    let img = link::link_many(store, &roots, base)?;
    let bin = format!("{out}.bin");
    std::fs::write(&bin, &img.bytes).map_err(|e| e.to_string())?;
    let file = std::path::Path::new(&bin)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let mut rs = format!(
        "// Generated by `chunker link`. Do not edit.\n\
         pub const BASE: u32 = {base:#x};\n\
         pub const IMAGE_LEN: usize = {};\n\
         pub static IMAGE: [u8; {}] = *include_bytes!(\"{file}\");\n",
        img.bytes.len(),
        img.bytes.len()
    );
    for (e, &r) in entries.iter().zip(&roots) {
        let a = img.addr[&store.canonical(r)];
        rs += &format!("pub const {}: u32 = {a:#x};\n", e.to_uppercase());
        println!("{e} at {a:#x}");
    }
    std::fs::write(format!("{out}.rs"), rs).map_err(|e| e.to_string())?;
    println!(
        "{} chunks, {} bytes -> {bin}",
        img.addr.len(),
        img.bytes.len()
    );
    Ok(())
}

/// Rounds 7b/7c: link without a base address. See `link::link_pic`.
fn link_pic_out(store: &Store, entries: &[String], out: &str, threshold: u32) -> Result<(), String> {
    let roots: Vec<usize> = entries
        .iter()
        .map(|e| store.by_name.get(e).copied().ok_or(format!("no entry {e}")))
        .collect::<Result<_, _>>()?;
    let img = link::link_pic(store, &roots, threshold)?;
    let w = |p: String, b: &[u8]| std::fs::write(&p, b).map_err(|e| format!("{p}: {e}"));
    w(format!("{out}.bin"), &img.flash)?;
    w(format!("{out}.static"), &img.gp.init)?;
    w(format!("{out}.bulk"), &img.bulk.init)?;
    let mut j = String::from("{\n  \"entries\": {");
    for (k, (e, &r)) in entries.iter().zip(&roots).enumerate() {
        let r = store.canonical(r);
        let (seg, off) = img.off.get(&r).ok_or(format!("{e} was not placed"))?;
        j += &format!("{}\n    \"{e}\": [\"{}\", {off}]", if k > 0 { "," } else { "" }, seg.name());
    }
    j += &format!(
        "\n  }},\n  \"flash_len\": {},\n  \"gp_size\": {},\n  \"gp_align\": {},\n  \"bulk_size\": {},\n  \"bulk_align\": {},\n  \"bulk_slots\": {},\n  \"threshold\": {threshold},\n  \"gp_rewrites\": {},\n  \"bulk_rewrites\": {},",
        img.flash.len(),
        img.gp.size,
        img.gp.align,
        img.bulk.size,
        img.bulk.align,
        img.bulk_slots,
        img.gp_rewrites,
        img.bulk_rewrites,
    );
    let relocs: Vec<String> = img
        .data_relocs
        .iter()
        .map(|(s, a, t, o)| format!("[\"{}\", {a}, \"{}\", {o}]", s.name(), t.name()))
        .collect();
    j += &format!("\n  \"data_relocs\": [{}],", relocs.join(", "));
    let placed: Vec<String> = img
        .placed
        .iter()
        .map(|p| format!("[{:?}, {}, \"{}\", {}, \"{}\"]", p.origin, p.size, p.seg.name(), p.off, p.why))
        .collect();
    j += &format!("\n  \"placed\": [{}],", placed.join(", "));
    let abs: Vec<String> = img.abs_flash.iter().map(|(c, o)| format!("[\"{c}\", {o}]")).collect();
    j += &format!("\n  \"abs_flash\": [{}]\n}}\n", abs.join(", "));
    w(format!("{out}.json"), j.as_bytes())?;
    let spilled = img.placed.iter().filter(|p| p.why == "spill").count();
    println!(
        "flash {} bytes, gp window {} bytes ({} slots), bulk {} bytes ({} statics, {} spilled), {} gp / {} bulk accesses, {} data relocs, {} absolute pointers in flash",
        img.flash.len(),
        img.gp.size,
        img.bulk_slots,
        img.bulk.size,
        img.placed.iter().filter(|p| p.seg == link::Seg::Bulk).count(),
        spilled,
        img.gp_rewrites,
        img.bulk_rewrites,
        img.data_relocs.len(),
        img.abs_flash.len()
    );
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((c, rest)) if !rest.is_empty() => (c.as_str(), rest),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    let (mut entries, mut label, mut inputs) = (Vec::new(), String::from("build"), Vec::new());
    let mut out = String::from("image");
    let mut threshold = mvm_abi::BULK_THRESHOLD;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--entry" => entries.push(it.next().expect("--entry needs a value").clone()),
            "--label" => label = it.next().expect("--label needs a value").clone(),
            "-o" => out = it.next().expect("-o needs a value").clone(),
            "--bulk-threshold" => {
                threshold = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--bulk-threshold needs a byte count")
            }

            _ => inputs.push(a.clone()),
        }
    }
    if cmd == "compare" {
        let split = inputs
            .iter()
            .position(|a| a == "--")
            .expect("compare needs `--` between builds");
        let res = load(&inputs[..split]).and_then(|mut a| {
            load(&inputs[split + 1..]).and_then(|mut b| report::compare(&mut a, &mut b, &entries))
        });
        if let Err(e) = res {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
        return;
    }
    let res = load(&inputs).and_then(|mut store| match cmd {
        "names" => report::names(&mut store),
        "link" => link_out(&store, &entries, &out),
        "link-pic" => link_pic_out(&store, &entries, &out, threshold),
        "applet-roundtrip" => applet_roundtrip(&store),
        "manifest" => report::manifest(&mut store, &label, &entries),
        "dump" => {
            dump(&mut store);
            Ok(())
        }
        "hashes" => {
            hashes(&mut store);
            match store.missing.is_empty() {
                true => Ok(()),
                false => Err(format!("unresolved symbols: {:?}", store.missing)),
            }
        }
        "roundtrip" => roundtrip(&store),
        _ => Err(USAGE.to_string()),
    });
    if let Err(e) = res {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
