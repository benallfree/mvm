mod chunk;
mod emu;
mod link;
mod rv;

use chunk::{Kind, Store, Target, display_name, hex};

const USAGE: &str = "usage:
  chunker dump      <objects/rlibs...>   every chunk, its slots and hash, plus a relocation report
  chunker hashes    <objects/rlibs...>   one line per named chunk and the anonymous chunks it uses (for diffing)
  chunker roundtrip <objects/rlibs...>   link spike_entry/spike_entry2, run them in the emulator, compare to native";

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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, inputs) = match args.split_first() {
        Some((c, rest)) if !rest.is_empty() => (c.as_str(), rest),
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };
    let res = load(inputs).and_then(|mut store| match cmd {
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
