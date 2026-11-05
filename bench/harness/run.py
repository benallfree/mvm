"""Run the bench firmware under Unicorn (Thumb, Cortex-M) and estimate cycles.

Every host instruction is hooked. Each one gets a cost from m4_cost.py, and
each instruction inside the interpreter is attributed to a phase (fetch,
decode, dispatch, execute, loop) via DWARF line info: the innermost inlined
frame in bench/interp/src gives a source line, and the closest
`// @phase:` marker above that line names the phase.

Usage: python3 run.py <fw elf> <guest counts file> <out.md>
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
from unicorn.arm_const import UC_ARM_REG_CPSR, UC_ARM_REG_LR, UC_ARM_REG_R0, UC_ARM_REG_SP

import m4_cost

HERE = os.path.dirname(os.path.abspath(__file__))
INTERP_SRC = os.path.normpath(os.path.join(HERE, "..", "interp", "src"))
RET = 0x1000_0000  # return address: execution stops here
PHASES = ["setup", "loop", "fetch", "decode", "dispatch", "execute", "wrapper"]


def load_elf(path):
    with open(path, "rb") as f:
        elf = ELFFile(f)
        segs = [(s["p_vaddr"], s.data(), s["p_memsz"]) for s in elf.iter_segments() if s["p_type"] == "PT_LOAD"]
        syms = {}
        funcs = []
        for s in elf.get_section_by_name(".symtab").iter_symbols():
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
        out[os.path.join(INTERP_SRC, fn)] = marks
    return out


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
            fn = os.path.normpath(fr.get("FileName", ""))
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

    def call(self, name):
        self.trace = []
        uc = self.uc
        uc.reg_write(UC_ARM_REG_SP, 0x2004_0000)
        uc.reg_write(UC_ARM_REG_LR, RET | 1)
        uc.emu_start(self.syms[name] | 1, RET, count=50_000_000)
        return uc.reg_read(UC_ARM_REG_R0), self.trace


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


def main():
    elf, counts_file, out_md = sys.argv[1:4]
    guest = {}
    with open(counts_file) as f:
        for line in f:
            mm = re.search(r"(simple|pd cold|pd warm) (crc16|parse): (0x[0-9a-f]+), (\d+) guest", line)
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

    interp_addrs = sorted({a for k, (_, tr) in runs.items() if k[0] != "native" for a, _ in tr
                           if "interp" in m.func_of(a)})
    phases = symbolize(elf, interp_addrs)

    res = {}
    bad = []
    for k, (ret, tr) in runs.items():
        want = guest[("simple", k[1])][0]
        if ret != want:
            bad.append(f"{k}: returned {ret:#x}, expected {want:#x}")
        res[k] = account(m, tr, None if k[0] == "native" else phases)
    if bad:
        raise SystemExit("wrong results:\n" + "\n".join(bad))

    L = []
    L.append("# Interpreter slowdown estimate (Unicorn + Cortex-M4 cost table)\n")
    L.append("Generated by `bench/run.sh`. Cycle costs: [COSTS.md](COSTS.md). "
             "Every result was checked against native.\n")
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
    for interp in ("simple", "pd warm", "pd cold"):
        for w in ("crc16", "parse"):
            g = guest[(interp, w)][1]
            t = res[(interp, w)][0]
            nat = res[("native", w)][0]["table"]
            L.append(f"| {interp} | {w} | {t['n'] / g:.1f} | {t['table'] / g:.1f} | {nat / g:.2f} |")
    L.append("")
    L.append("Native host instructions: " + ", ".join(
        f"{w} {res[('native', w)][0]['n']}" for w in ("crc16", "parse")) + ".\n")
    L.append("## Where the interpreter spends cycles (table model)\n")
    for interp in ("simple", "pd warm"):
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
    L.append("Cycles by instruction class, simple interpreter, crc16:\n")
    cls = res[("simple", "crc16")][0]["cls"]
    tot = sum(cls.values())
    L.append("| Class | Cycles | % |")
    L.append("|---|---|---|")
    for c, v in cls.most_common():
        L.append(f"| `{c}` | {v} | {100 * v / tot:.1f}% |")
    L.append("")
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
