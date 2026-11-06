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
from unicorn import arm_const as A  # noqa: E402

SECTIONS = ["load_all", "empty_loop", "nop_loop", "uart_loop_muted", "local_loop_len0", "echo_loop_len0",
            "local_loop_len64", "echo_loop_len64", "recurse_depth1", "recurse_depth6", "call_overhead",
            "none_access_loop", "gp_access_loop", "bulk_access_loop",
            # Round 7d (tests::asynct::SECTIONS)
            "a_budget_burn", "a_long_call", "a_call_empty", "a_empty_loop", "a_check_loop", "a_start16",
            "a_cancel16", "a_select_poll", "a_wake_poll"]
# Round 7d: the driver interrupt. Every IRQ_PERIOD estimated cycles (1 ms at
# 64 MHz) the harness saves the registers, runs `mo_irq` (returning to
# IRQ_RET) and restores them, as an exception entry/exit would. Never inside
# an IT block. `mo_idle` (the firmware's WFI) skips ahead to the next tick.
IRQ_PERIOD = 64_000
IRQ_RET = thumb.RET + 0x100
SAVED = [getattr(A, f"UC_ARM_REG_R{i}") for i in range(13)] + [A.UC_ARM_REG_SP, A.UC_ARM_REG_LR,
                                                               A.UC_ARM_REG_PC, A.UC_ARM_REG_XPSR]
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
    irq, idle, irq_on = syms["mo_irq"], syms["mo_idle"], syms["MO_IRQ_ON"]
    st = {"cyc": 0, "n": 0, "prev": None, "after_ldst": False, "next_irq": IRQ_PERIOD, "saved": None,
          "irqs": 0, "irq_cyc": 0, "idle_cyc": 0, "irq_start": 0}
    marks = []

    def hook(u, addr, size, _):
        if addr == IRQ_RET:  # the handler returned: restore the interrupted context
            for r, v in zip(SAVED, st["saved"]):
                u.reg_write(r, v)
            st["irq_cyc"] += st["cyc"] - st["irq_start"]
            st["resumed"] = st["saved"][15] & ~1
            st["saved"], st["prev"] = None, None
            return
        resumed, st["resumed"] = st.get("resumed") == addr, None
        # Idle: jump to the next tick, but not when we just came back here from
        # that tick's handler (the firmware must get to run after it).
        if addr == idle and not resumed and st["saved"] is None and st["cyc"] < st["next_irq"]:
            st["idle_cyc"] += st["next_irq"] - st["cyc"]
            st["cyc"] = st["next_irq"]
        if st["cyc"] >= st["next_irq"] and st["saved"] is None:
            i = code.get(addr)
            inside_it = i is not None and i.conditional and i.cls not in ("bcc", "cb")
            if not inside_it:
                st["next_irq"] = max(st["next_irq"] + IRQ_PERIOD, st["cyc"] - st["cyc"] % IRQ_PERIOD + IRQ_PERIOD)
                if int.from_bytes(u.mem_read(irq_on, 4), "little"):
                    p = st["prev"]
                    if p is not None:
                        pi, ok = p
                        st["cyc"] += m4_cost.cost(pi, ok, False, st["after_ldst"], False)
                    st["saved"] = [u.reg_read(r) for r in SAVED]
                    st["saved"][15] = addr | 1  # resume here
                    # Exception entry: 12 cycles, frame pushed below the interrupted stack.
                    st["cyc"] += 12
                    st["irq_start"] = st["cyc"]
                    st["irqs"] += 1
                    u.reg_write(A.UC_ARM_REG_SP, (st["saved"][13] - 64) & ~7)
                    u.reg_write(A.UC_ARM_REG_LR, IRQ_RET | 1)
                    u.reg_write(A.UC_ARM_REG_PC, irq | 1)
                    st["prev"], st["after_ldst"] = None, False
                    return
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
    ok = thumb.call(uc, syms["mo_run"], count=2_000_000_000)

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
           "max_host_stack_bytes": STACK_TOP - (bss_end + deepest), "total_instrs": st["n"],
           "irqs": st["irqs"], "irq_cycles": st["irq_cyc"], "idle_cycles": st["idle_cyc"], "total_cycles": st["cyc"]}
    json.dump(res, open(out, "w"), indent=1)
    print(console, end="")
    print(f"unicorn: {st['n']} host instructions, max host stack {res['max_host_stack_bytes']} bytes, "
          f"{st['irqs']} driver interrupts ({st['irq_cyc']} cycles), {st['idle_cyc']} idle cycles skipped")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
