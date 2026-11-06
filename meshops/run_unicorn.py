#!/usr/bin/env python3
"""Run the round-seven mesh-op tests under Unicorn (Cortex-M, Thumb) and
estimate cycles with the round-three Cortex-M4 cost table.

Instructions are costed as they execute (no trace is kept). Each call to
`mo_mark` records the running totals; the shared test runner brackets every
measured section with exactly two counter reads, in tests::SECTIONS order.

Writes a JSON summary (console text + per-section cycles/instructions) and
prints the console. Usage: run_unicorn.py <fw elf> <out.json>
"""
import json, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "bench", "harness"))
import m4_cost  # noqa: E402
import thumb  # noqa: E402
from unicorn import UC_HOOK_CODE  # noqa: E402

SECTIONS = ["load_all", "empty_loop", "nop_loop", "uart_loop_muted", "local_loop_len0", "echo_loop_len0",
            "local_loop_len64", "echo_loop_len64", "recurse_depth1", "recurse_depth6", "call_overhead",
            "none_access_loop", "gp_access_loop", "bulk_access_loop"]
STACK_TOP = thumb.STACK_TOP
PAINT = 0xA5


def main():
    elf, out = sys.argv[1:3]
    segs, syms, _funcs, code = thumb.load_elf(elf)
    uc = thumb.new_unicorn(segs)
    # Paint the stack region to measure the deepest host stack use afterwards.
    bss_end = max(v + m for v, _, m in segs if v >= 0x2000_0000)
    uc.mem_write(bss_end, bytes([PAINT]) * (STACK_TOP - bss_end))

    mark = syms["mo_mark"]
    st = {"cyc": 0, "n": 0, "prev": None, "after_ldst": False}
    marks = []

    def hook(u, addr, size, _):
        p = st["prev"]
        if p is not None:
            i, ok = p
            taken = addr != i.addr + i.size
            st["cyc"] += m4_cost.cost(i, ok, taken, st["after_ldst"], False)
            st["after_ldst"] = ok and i.cls in ("ld", "st")
        i = code[addr]
        st["prev"] = (i, thumb.executed(i, u))
        st["n"] += 1
        if addr == mark:
            marks.append((st["cyc"], st["n"]))

    uc.hook_add(UC_HOOK_CODE, hook)
    ok = thumb.call(uc, syms["mo_run"], count=500_000_000)

    n = int.from_bytes(uc.mem_read(syms["MO_CONSOLE_LEN"], 4), "little")
    console = bytes(uc.mem_read(syms["MO_CONSOLE"], n)).decode(errors="replace")
    painted = bytes(uc.mem_read(bss_end, STACK_TOP - bss_end))
    deepest = next((k for k, b in enumerate(painted) if b != PAINT), len(painted))
    if len(marks) != 2 * len(SECTIONS):
        sys.exit(f"expected {2 * len(SECTIONS)} marks, got {len(marks)}")
    sections = {}
    for k, name in enumerate(SECTIONS):
        (c0, n0), (c1, n1) = marks[2 * k], marks[2 * k + 1]
        sections[name] = {"cycles": c1 - c0, "instrs": n1 - n0}
    res = {"pass": bool(ok), "console": console, "sections": sections,
           "max_host_stack_bytes": STACK_TOP - (bss_end + deepest), "total_instrs": st["n"]}
    json.dump(res, open(out, "w"), indent=1)
    print(console, end="")
    print(f"unicorn: {st['n']} host instructions, max host stack {res['max_host_stack_bytes']} bytes")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
