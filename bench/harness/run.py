"""Run the bench firmware under Unicorn (Thumb, Cortex-M) and estimate cycles.

Every host instruction is hooked. Each one gets a cost from m4_cost.py, and
each instruction inside the interpreter is attributed to a phase (fetch,
decode, dispatch, execute, loop) via DWARF line info: the innermost inlined
frame in bench/interp/src gives a source line, and the closest
`// @phase:` marker above that line names the phase.

Usage: python3 run.py <fw elf> <guest counts file> <out.md> [<aot.bin> <aot.syms>]

With the AOT blob (round four), the translated code is placed at AOT_AT, an
arbitrary aligned flash address, checked against native on 5 inputs, timed,
and costed per translator category (aotc_<cat>_<n> labels).
"""

import bisect
import collections
import json
import os
import re
import subprocess
import sys

from elftools.elf.elffile import ELFFile
from unicorn import UC_ARCH_ARM, UC_HOOK_CODE, UC_MODE_MCLASS, UC_MODE_THUMB, Uc
from unicorn.arm_const import (UC_ARM_REG_CPSR, UC_ARM_REG_LR, UC_ARM_REG_R0, UC_ARM_REG_R1,
                                UC_ARM_REG_R2, UC_ARM_REG_R3, UC_ARM_REG_SP)

import m4_cost

HERE = os.path.dirname(os.path.abspath(__file__))
INTERP_SRC = os.path.normpath(os.path.join(HERE, "..", "interp", "src"))
RET = 0x1000_0000  # return address: execution stops here
AOT_AT = 0x000C_4000  # where the translated blob runs from (any 4-aligned flash address)
AOT_CATS = ["stub", "op", "spill", "const", "flags", "shiftmask", "addr"]
PHASES = ["setup", "loop", "fetch", "decode", "dispatch", "execute", "wrapper"]


def load_elf(path):
    with open(path, "rb") as f:
        elf = ELFFile(f)
        segs = [(s["p_vaddr"], s.data(), s["p_memsz"]) for s in elf.iter_segments() if s["p_type"] == "PT_LOAD"]
        syms = {}
        funcs = []
        for s in elf.get_section_by_name(".symtab").iter_symbols():
            if s["st_info"]["type"] == "STT_OBJECT":
                syms[s.name] = s["st_value"]
            if s["st_info"]["type"] == "STT_FUNC" and s["st_size"]:
                a = s["st_value"] & ~1
                syms[s.name] = a
                funcs.append((a, s["st_size"], s.name))
        code = {}
        for a, size, _ in funcs:
            sec = next(sec for sec in elf.iter_sections() if sec["sh_addr"] <= a < sec["sh_addr"] + sec["sh_size"])
            off = a - sec["sh_addr"]
            data = sec.data()[off:off + size]
            # Decode each function from its start so capstone tracks IT blocks.
            for ci in m4_cost.MD.disasm(data, a):
                code[ci.address] = m4_cost.Insn(ci)
    return segs, syms, sorted(funcs), code


def phase_markers():
    """file -> sorted [(line, phase)] from `// @phase: name` comments."""
    out = {}
    for fn in os.listdir(INTERP_SRC):
        if not fn.endswith(".rs"):
            continue
        marks = []
        with open(os.path.join(INTERP_SRC, fn)) as f:
            for no, line in enumerate(f, 1):
                m = re.search(r"//\s*@phase:\s*(\w+)", line)
                if m:
                    marks.append((no, m.group(1)))
        out[fn] = marks
    return out


def interp_file(path):
    """`lib.rs` etc. for a file under .../interp/src/, else None. Matching on the
    suffix keeps attribution independent of where the repo is checked out (the
    Docker image mounts it at /work)."""
    norm = path.replace("\\", "/")
    if "/interp/src/" not in norm:
        return None
    return norm.rsplit("/interp/src/", 1)[1]


def symbolize(elf, addrs):
    """addr -> phase, from the innermost frame inside bench/interp/src."""
    marks = phase_markers()
    inp = "\n".join(hex(a) for a in addrs) + "\n"
    r = subprocess.run(["llvm-symbolizer", f"--obj={elf}", "--inlining", "--output-style=JSON"],
                       input=inp, capture_output=True, text=True, check=True)
    out = {}
    for line in r.stdout.splitlines():
        if not line.strip():
            continue
        e = json.loads(line)
        a = int(e["Address"], 16)
        phase = None  # line 0: compiler-generated (spills, merges), resolved below
        for fr in e["Symbol"]:  # innermost first
            fn = interp_file(fr.get("FileName", ""))
            if fn in marks and fr["Line"] > 0:
                ms = marks[fn]
                i = bisect.bisect_right([m[0] for m in ms], fr["Line"]) - 1
                phase = ms[i][1] if i >= 0 else "setup"
                break
            if fr["Line"] == 0:
                break
        out[a] = phase
    # Line-0 code takes the phase of the closest preceding attributed instruction.
    last = "setup"
    for a in sorted(out):
        if out[a] is None:
            out[a] = last
        else:
            last = out[a]
    return out


class Machine:
    def __init__(self, elf):
        self.elf = elf
        segs, self.syms, self.funcs, self.code = load_elf(elf)
        self.uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
        self.uc.mem_map(0x0, 0x10_0000)  # flash
        self.uc.mem_map(0x2000_0000, 0x4_0000)  # RAM
        self.uc.mem_map(RET, 0x1000)
        for vaddr, data, memsz in segs:
            self.uc.mem_write(vaddr, data + b"\0" * (memsz - len(data)))
        self.trace = []
        self.uc.hook_add(UC_HOOK_CODE, self._hook)
        self._starts = [f[0] for f in self.funcs]

    def _hook(self, uc, addr, size, _):
        i = self.code.get(addr)
        ok = True
        if i is not None and i.conditional and i.cls not in ("bcc", "cb"):
            ok = m4_cost.cond_passes(i.cc, uc.reg_read(UC_ARM_REG_CPSR))
        self.trace.append((addr, ok))

    def func_of(self, addr):
        k = bisect.bisect_right(self._starts, addr) - 1
        a, size, name = self.funcs[k]
        return name if a <= addr < a + size else "?"

    def call(self, name, args=()):
        self.trace = []
        uc = self.uc
        uc.reg_write(UC_ARM_REG_SP, 0x2004_0000)
        uc.reg_write(UC_ARM_REG_LR, RET | 1)
        for r, v in zip((UC_ARM_REG_R0, UC_ARM_REG_R1, UC_ARM_REG_R2, UC_ARM_REG_R3), args):
            uc.reg_write(r, v)
        uc.emu_start(self.syms[name] | 1, RET, count=50_000_000)
        return uc.reg_read(UC_ARM_REG_R0), self.trace

    def load_aot(self, path, syms_path, at):
        """Place translated code at `at` (any 4-aligned flash address) and decode it."""
        blob = open(path, "rb").read()
        self.uc.mem_write(at, blob)
        labels = []
        for line in open(syms_path):
            parts = line.split()
            if len(parts) == 3:
                labels.append((int(parts[0], 16) + at, parts[2]))
        labels.sort()
        for a, n in labels:
            if n.startswith("aot_"):
                self.syms[n] = a
        code_end = self.syms["aot_image"]
        for ci in m4_cost.MD.disasm(blob[: code_end - at], at):
            self.code[ci.address] = m4_cost.Insn(ci)
        self.funcs.append((at, code_end - at, "aot"))
        self.funcs.sort()
        self._starts = [f[0] for f in self.funcs]
        # address -> category, from the aotc_<cat>_<n> labels
        cats = [(a, n.split("_")[1]) for a, n in labels if n.startswith("aotc_")]
        self.aot_cats = cats
        return len(blob), code_end - at


def account(m, trace, phases=None):
    """Totals and per-phase costs for one traced call."""
    t = dict(n=0, table=0, pipe=0, unmodeled=collections.Counter(), cls=collections.Counter())
    ph = collections.defaultdict(lambda: [0, 0])  # phase -> [instructions, table cycles]
    after_ldst = False
    for k, (addr, ok) in enumerate(trace):
        i = m.code.get(addr)
        if i is None:
            raise SystemExit(f"no decode for executed address {addr:#x} in {m.func_of(addr)}")
        nxt = trace[k + 1][0] if k + 1 < len(trace) else RET
        taken = nxt != addr + i.size
        c_tab = m4_cost.cost(i, ok, taken, after_ldst, False)
        c_pipe = m4_cost.cost(i, ok, taken, after_ldst, True)
        after_ldst = ok and i.cls in ("ld", "st")
        t["n"] += 1
        t["table"] += c_tab
        t["pipe"] += c_pipe
        t["cls"][i.cls if ok else "skipped"] += c_tab
        if not i.modeled:
            t["unmodeled"][i.name] += 1
        if phases is not None:
            p = phases.get(addr, "wrapper")
            ph[p][0] += 1
            ph[p][1] += c_tab
    return t, ph


def rv_code_bytes(syms_path):
    """RV32IMC code bytes reachable from the entries, as counted by the translator."""
    info = open(os.path.join(os.path.dirname(syms_path), "info.txt")).read()
    return int(re.search(r"guest code bytes: (\d+)", info).group(1))


def main():
    elf, counts_file, out_md = sys.argv[1:4]
    aot = sys.argv[4:6] if len(sys.argv) >= 6 else None
    guest = {}
    with open(counts_file) as f:
        for line in f:
            mm = re.search(r"(simple|pd cold|pd warm|fast cold|fast warm) (crc16|parse): (0x[0-9a-f]+), (\d+) guest", line)
            if mm:
                guest[(mm.group(1), mm.group(2))] = (int(mm.group(3), 16), int(mm.group(4)))

    m = Machine(elf)
    runs = {}
    m.call("bench_setup")
    for w in ("crc16", "parse"):
        runs[("native", w)] = m.call(f"bench_native_{w}")
        runs[("simple", w)] = m.call(f"bench_simple_{w}")
    m.call("bench_setup")  # empty pre-decode table
    for w in ("crc16", "parse"):
        runs[("pd cold", w)] = m.call(f"bench_pd_{w}")
        runs[("pd warm", w)] = m.call(f"bench_pd_{w}")
        runs[("fast cold", w)] = m.call(f"bench_fast_{w}")
        runs[("fast warm", w)] = m.call(f"bench_fast_{w}")

    if aot:
        blob_size, aot_code = m.load_aot(aot[0], aot[1], AOT_AT)
        sym = lambda frag: next(v for k, v in m.syms.items() if frag in k)
        # Correctness: the 5 inputs from the host tests, native vs AOT, same buffer.
        crc, pkt = bytes(m.uc.mem_read(sym("CRC_IN"), 256)), bytes(m.uc.mem_read(sym("PKT_IN"), 64))
        bad = bytearray(pkt)
        bad[63] ^= 1
        cases = [("crc16", crc), ("crc16", b"123456789"), ("parse", pkt), ("parse", bytes(bad)), ("parse", pkt[:20])]
        aot_checks = []
        for w, data in cases:
            buf = 0x2003_0000
            m.uc.mem_write(buf, data)
            want, _ = m.call(w, (buf, len(data)))
            got, _ = m.call(f"aot_{w}", (buf, len(data)))
            aot_checks.append((w, len(data), want, got))
        wrong = [c for c in aot_checks if c[2] != c[3]]
        if wrong:
            raise SystemExit(f"AOT mismatch: {wrong}")
        runs[("aot", "crc16")] = m.call("aot_crc16", (sym("CRC_IN"), 256))
        runs[("aot", "parse")] = m.call("aot_parse", (sym("PKT_IN"), 64))
        cat_addrs = [a for a, _ in m.aot_cats]
        aot_phase = {}
        for k in (("aot", "crc16"), ("aot", "parse")):
            for a, _ in runs[k][1]:
                i = bisect.bisect_right(cat_addrs, a) - 1
                aot_phase[a] = m.aot_cats[i][1] if i >= 0 else "stub"

    interp_addrs = sorted({a for k, (_, tr) in runs.items() if k[0] != "native" for a, _ in tr
                           if "interp" in m.func_of(a)})
    phases = symbolize(elf, interp_addrs)

    res = {}
    bad = []
    for k, (ret, tr) in runs.items():
        want = guest[("simple", k[1])][0]
        if ret != want:
            bad.append(f"{k}: returned {ret:#x}, expected {want:#x}")
        res[k] = account(m, tr, None if k[0] == "native" else aot_phase if k[0] == "aot" else phases)
    if bad:
        raise SystemExit("wrong results:\n" + "\n".join(bad))

    L = []
    L.append("# Interpreter slowdown estimate (Unicorn + Cortex-M4 cost table)\n")
    L.append("Generated by `bench/run.sh`. Cycle costs: [COSTS.md](COSTS.md). "
             "Every result was checked against native.\n")
    if aot:
        L.append("## Comparison\n")
        L.append("| Workload | Native | AOT | Slowdown | Fast interp (warm) | Slowdown | Pre-decoded (warm) | Slowdown | Simple | Slowdown |")
        L.append("|---|---|---|---|---|---|---|---|---|---|")
        for w in ("crc16", "parse"):
            nat = res[("native", w)][0]["table"]
            row = [str(nat)]
            for k in ("aot", "fast warm", "pd warm", "simple"):
                c = res[(k, w)][0]["table"]
                row += [str(c), f"**{c / nat:.2f}x**"]
            L.append(f"| {w} | " + " | ".join(row) + " |")
        L.append("")
        L.append(f"AOT correctness: {len(aot_checks)}/5 inputs match native (" + ", ".join(
            f"{w}[{n}] {g:#x}" for w, n, _, g in aot_checks) + f"). Run from {AOT_AT:#x}.\n")
        L.append("### Code size\n")
        fsz = {f[2]: f[1] for f in m.funcs}
        nat_sz = fsz["crc16"] + fsz["parse"]
        L.append("| Artifact | crc16 + parse code | Data |")
        L.append("|---|---|---|")
        L.append(f"| Native Thumb | {nat_sz} B (crc16 {fsz['crc16']}, parse {fsz['parse']}) | 512 B CRC table |")
        L.append(f"| RV32IMC image (code reachable from the entries) | {rv_code_bytes(aot[1])} B | 512 B CRC table (912 B image in all) |")
        L.append(f"| AOT Thumb | {aot_code} B (incl. stubs) | embedded image copy, {blob_size - aot_code} B with alignment |")
        L.append("")
        L.append("### Where AOT spends cycles (table model)\n")
        L.append("| Category | crc16 cycles | % | parse cycles | % |")
        L.append("|---|---|---|---|---|")
        for p in AOT_CATS:
            row = []
            for w in ("crc16", "parse"):
                t, ph = res[("aot", w)]
                c = ph.get(p, [0, 0])[1]
                row += [str(c), f"{100 * c / t['table']:.1f}%"]
            L.append(f"| {p} | " + " | ".join(row) + " |")
        L.append("")
        L.append("| Workload | Native host instrs | AOT host instrs | Guest instrs | AOT host instrs / guest instr |")
        L.append("|---|---|---|---|---|")
        for w in ("crc16", "parse"):
            g = guest[("simple", w)][1]
            L.append(f"| {w} | {res[('native', w)][0]['n']} | {res[('aot', w)][0]['n']} | {g} | {res[('aot', w)][0]['n'] / g:.2f} |")
        L.append("")
    L.append("## Summary\n")
    L.append("| Workload | Guest instrs | Native cycles | Simple cycles | Slowdown | Pre-decoded (warm) | Slowdown | Pre-decoded (cold) | Slowdown |")
    L.append("|---|---|---|---|---|---|---|---|---|")
    for w in ("crc16", "parse"):
        g = guest[("simple", w)][1]
        nat = res[("native", w)][0]["table"]
        s = res[("simple", w)][0]["table"]
        pw = res[("pd warm", w)][0]["table"]
        pc = res[("pd cold", w)][0]["table"]
        L.append(f"| {w} | {g} | {nat} | {s} | **{s / nat:.1f}x** | {pw} | **{pw / nat:.1f}x** | {pc} | {pc / nat:.1f}x |")
    L.append("")
    L.append("Pipelined load/store model (the TRM's best case) for comparison:\n")
    L.append("| Workload | Native | Simple | Slowdown | Pre-decoded warm | Slowdown |")
    L.append("|---|---|---|---|---|---|")
    for w in ("crc16", "parse"):
        nat = res[("native", w)][0]["pipe"]
        s = res[("simple", w)][0]["pipe"]
        pw = res[("pd warm", w)][0]["pipe"]
        L.append(f"| {w} | {nat} | {s} | {s / nat:.1f}x | {pw} | {pw / nat:.1f}x |")
    L.append("")
    L.append("## Per guest instruction\n")
    L.append("| Interpreter | Workload | Host instrs / guest instr | Cycles / guest instr | Native cycles / guest instr |")
    L.append("|---|---|---|---|---|")
    for interp in ("simple", "pd warm", "pd cold", "fast warm", "fast cold"):
        for w in ("crc16", "parse"):
            g = guest[(interp, w)][1]
            t = res[(interp, w)][0]
            nat = res[("native", w)][0]["table"]
            L.append(f"| {interp} | {w} | {t['n'] / g:.1f} | {t['table'] / g:.1f} | {nat / g:.2f} |")
    L.append("")
    L.append("Native host instructions: " + ", ".join(
        f"{w} {res[('native', w)][0]['n']}" for w in ("crc16", "parse")) + ".\n")
    L.append("## Where the interpreter spends cycles (table model)\n")
    for interp in ("simple", "pd warm", "fast warm"):
        L.append(f"**{interp}**\n")
        L.append("| Phase | " + " | ".join(f"{w} cycles | %" for w in ("crc16", "parse")) + " | cycles / guest instr (crc16) |")
        L.append("|---|---|---|---|---|---|")
        for p in PHASES:
            row = []
            for w in ("crc16", "parse"):
                t, ph = res[(interp, w)]
                c = ph.get(p, [0, 0])[1]
                row += [str(c), f"{100 * c / t['table']:.1f}%"]
            per = res[(interp, "crc16")][1].get(p, [0, 0])[1] / guest[(interp, "crc16")][1]
            if any(res[(interp, w)][1].get(p, [0, 0])[1] for w in ("crc16", "parse")):
                L.append(f"| {p} | " + " | ".join(row) + f" | {per:.1f} |")
        L.append("")
    L.append("Cycles by instruction class, crc16 (per guest instruction in brackets):\n")
    L.append("| Class | Simple | Fast (warm) |")
    L.append("|---|---|---|")
    cs, cf = res[("simple", "crc16")][0]["cls"], res[("fast warm", "crc16")][0]["cls"]
    g = guest[("simple", "crc16")][1]
    for c in sorted(set(cs) | set(cf), key=lambda c: -cs.get(c, 0)):
        L.append(f"| `{c}` | {cs.get(c, 0)} ({cs.get(c, 0) / g:.1f}) | {cf.get(c, 0)} ({cf.get(c, 0) / g:.1f}) |")
    L.append("")
    L.append("The fast interpreter's phase table is less exact than the others': the compiler")
    L.append("moves field extraction into the arms and a shared tail, so line-based phases blur.")
    L.append("The class table above is exact.\n")
    um = collections.Counter()
    for t, _ in res.values():
        um.update(t["unmodeled"])
    L.append(f"Instructions outside the cost table (counted as 1 cycle): {dict(um) or 'none'}.\n")
    text = "\n".join(L)
    with open(out_md, "w") as f:
        f.write(text)
    print(text)


if __name__ == "__main__":
    main()
