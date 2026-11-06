#!/usr/bin/env python3
"""Write meshops/RESULTS.md from the round-seven/7b outputs:
out/host.txt (x86), out/unicorn.json (Unicorn + M4 cost table), and the
emulator UART logs in emu/out/ (Renode nRF52840, QEMU ESP32-S3) when present.
"""
import json, os, re

HERE = os.path.dirname(os.path.abspath(__file__))
EMU = os.path.join(HERE, "..", "emu", "out")
TESTS = ["load", "1-hello", "2-timer", "3-nop-cost", "4-applet-op", "5-nested", "6-missing-import", "7-swap", "8-spoof",
         "9-position-independence", "10-instances", "11-real-pointers", "12-shared-stack", "13-fit",
         "14-big-buffer", "15-overflow-spill", "16-bulk-position-independence", "17-bulk-real-pointer", "18-bulk-fit",
         "19-no-annotations",
         "async-load", "20-hello-loop", "21-two-tasks", "22-two-applets", "23-idle", "24-no-deadlock", "25-budget",
         "26-stale-ticket", "27-inbox-overflow", "28-driver-priority", "29-select", "async-costs", "async-sizes",
         "30-script-inputs"]
# Test 30 needs an outside script typing on the UART: emulators only.
EMU_ONLY = {"30-script-inputs"}
# Thumb: run_guest's prologue (9 saved registers + 0xc4); dispatch is inlined and
# run_guest calls itself directly. Round seven: 464 B (run_guest 392 + dispatch 72).
HOST_STACK_PER_LEVEL = 232
# Round seven, Unicorn (Cortex-M4 cost table): (cycles, ×guest), from its RESULTS.md.
ROUND7_UNICORN = {
    "kernel.nop.v1": (216, 7.5),
    "kernel.uart.write.v1 (16 B, console muted)": (670, 23.1),
    "applet op test.echo.v1 (len 0, A's work ~1 instr)": (729, 25.1),
    "each extra nesting level (test.recurse.v1)": (902, 31.1),
    "host -> guest entry (Kernel::call, empty body)": (564, 19.4),
}
ROUND7_GUEST = 29.0
# Guest instructions a level of test.recurse.v1 runs (A's own code, from the
# disassembly): round seven had no frame; 7b fills a 32-byte frame so the level
# shows on the shared stack.
RECURSE_INSTRS = {"round 7": 7, "7b": 28}
APPLETS = ["h", "a", "a2", "b", "m", "s1", "s2", "s3", "c", "d", "e"]


def parse(text, chip):
    tests, costs, info = {}, {}, {}
    for line in text.splitlines():
        m = re.match(rf"MO {chip} (\S+) pass=([01])\s*(.*)", line)
        if m:
            tests[m.group(1)] = (m.group(2) == "1", m.group(3))
        m = re.match(rf"MO {chip} cost (\S+) n=(\d+) cycles=(\d+) steps=(\d+)", line)
        if m:
            costs[m.group(1)] = {"n": int(m.group(2)), "cycles": int(m.group(3)), "steps": int(m.group(4))}
        m = re.match(rf"MO {chip} load pass=1 (.*)", line)
        if m:
            info = dict(kv.split("=") for kv in m.group(1).split())
    return tests, costs, info


def per_call(c, n_empty_iters_steps=5):
    """Per-call costs in the target's own units, and in guest-instruction equivalents."""
    n = c["empty_loop"]["n"]
    g = c["empty_loop"]["cycles"] / (n_empty_iters_steps * n + 7)  # one guest instruction
    rows = {
        "kernel.nop.v1": (c["nop_loop"]["cycles"] - c["empty_loop"]["cycles"]) / n,
        "kernel.uart.write.v1 (16 B, console muted)": (c["uart_loop_muted"]["cycles"] - c["empty_loop"]["cycles"]) / n,
        "applet op test.echo.v1 (len 0, A's work ~1 instr)": (c["echo_loop_len0"]["cycles"] - c["local_loop_len0"]["cycles"]) / c["echo_loop_len0"]["n"],
        "each extra nesting level (test.recurse.v1)": (c["recurse_depth6"]["cycles"] - c["recurse_depth1"]["cycles"]) / 5 / c["recurse_depth6"]["n"],
        "host -> guest entry (Kernel::call, empty body)": c["call_overhead"]["cycles"] / c["call_overhead"]["n"],
    }
    return g, rows, c["load_all"]["cycles"] / 3


def manifests():
    """Per applet: (gp window size, bulk size, bulk slots) from the generated applets.rs."""
    src = open(os.path.join(HERE, "guest-image", "applets.rs")).read()
    out = {}
    for m in re.finditer(r"pub mod (\w+) \{(.*?)\n\}", src, re.S):
        body = m.group(2)
        g = re.search(r"gp: Segment \{ size: (\d+)", body)
        b = re.search(r"bulk: Segment \{ size: (\d+)", body)
        k = re.search(r"bulk_slots: (\d+)", body)
        out[m.group(1)] = (int(g.group(1)), int(b.group(1)), int(k.group(1)))
    return out


def async_costs(c, g):
    """Round 7d: per-op costs (target units) and the guest-instruction equivalents."""
    n = c["a_check_loop"]["n"]
    one_call = c["a_call_empty"]["cycles"] / c["a_call_empty"]["n"]
    rows = {
        "kernel.ticket.check.v1 (pending ticket)": (c["a_check_loop"]["cycles"] - c["a_empty_loop"]["cycles"]) / n,
        "kernel.timer.start.v1": (c["a_start16"]["cycles"] - one_call) / 16,
        "kernel.ticket.cancel.v1 (pending timer)": (c["a_cancel16"]["cycles"] - one_call) / 16,
        "one select poll (kernel poll of a task in Select2, both pending)": c["a_select_poll"]["cycles"] / c["a_select_poll"]["n"],
        "wake-and-poll round trip (GPIO event in -> task resumed and re-armed)": c["a_wake_poll"]["cycles"] / c["a_wake_poll"]["n"],
    }
    return rows


def access_costs(c):
    """Round 7c: one read-modify-write of a gp-window / bulk static, net of the
    same call loop with no access."""
    n = c["none_access_loop"]["n"]
    base = c["none_access_loop"]["cycles"]
    return {k: (c[f"{k}_access_loop"]["cycles"] - base) / n for k in ("gp", "bulk")}


def main():
    targets = []
    host = open(os.path.join(HERE, "out", "host.txt")).read()
    targets.append(("x86 host", "host", *parse(host, "host"), "ns (wall clock)"))
    uj = json.load(open(os.path.join(HERE, "out", "unicorn.json")))
    t, c, i = parse(uj["console"], "unicorn")
    for k, v in uj["sections"].items():
        c[k]["cycles"] = v["cycles"]
    targets.append(("Unicorn (Cortex-M4 cost table)", "unicorn", t, c, i, "est. M4 cycles"))
    for chip, label, unit in [("nrf52840", "Renode nRF52840", "DWT units (= instrs x 0.64)"),
                              ("esp32s3", "QEMU ESP32-S3", "CCOUNT under -icount")]:
        p = os.path.join(EMU, f"{chip}.uart")
        if os.path.exists(p):
            text = open(p, errors="replace").read()
            # A log from an interrupted run has no cost lines: leave it out.
            if f"MO {chip} done" in text:
                targets.append((label, chip, *parse(text, chip), unit))

    L = ["# Round 7d results: mesh ops, memory model, bulk statics, async events", "",
         "Generated by `meshops/report.py` (run by `meshops/run.sh`). Design and ABI: [MESHOPS.md](../MESHOPS.md); "
         "memory model: [MEMMODEL.md](../MEMMODEL.md); bulk statics: [BULK.md](../BULK.md); async: [ASYNC.md](../ASYNC.md). "
         "Tests 1-8 are round seven's, 9-13 round 7b's, 14-19 round 7c's, 20-30 round 7d's.", ""]
    L += ["## Pass/fail", "", "| Test | " + " | ".join(t[0] for t in targets) + " |",
          "|---|" + "---|" * len(targets)]
    allpass = True
    for name in TESTS:
        row = []
        for t in targets:
            if name in EMU_ONLY and t[1] in ("host", "unicorn"):
                row.append("n/a")
                continue
            ok, detail = t[2].get(name, (False, ""))
            allpass &= ok
            row.append("skipped" if ok and "skipped=1" in detail else "✅" if ok else "❌")
        L.append(f"| {name} | " + " | ".join(row) + " |")
    L += ["", f"Overall: {'all pass' if allpass else 'FAILURES'}.", "",
          "*skipped*: test 28 needs drivers in a context that preempts guest code; on QEMU ESP32-S3 every interrupt "
          "source tried hung the emulated CPU, so the drivers tick from the interpreter's wait loop there (see ASYNC.md). "
          "*n/a*: test 30 needs an outside script typing on a UART (emulators only).", ""]

    L += ["## Call cost", "",
          "Each figure is the extra cost of one call over the same loop without it, "
          "so loop overhead cancels. **×guest** divides by the cost of one interpreted "
          "guest instruction measured in the same run (the empty loop), which doesn't depend on "
          "how each emulator's counter maps to real cycles.", ""]
    hdr = "| Call | " + " | ".join(f"{t[0]} | ×guest" for t in targets) + " |"
    L += [hdr, "|---|" + "---|---|" * len(targets)]
    calc = [per_call(t[3]) for t in targets]
    for row in calc[0][1]:
        cells = []
        for (g, rows, _), t in zip(calc, targets):
            cells += [f"{rows[row]:.0f}", f"{rows[row] / g:.1f}"]
        L.append(f"| {row} | " + " | ".join(cells) + " |")
    cells = []
    for (g, _, _), t in zip(calc, targets):
        cells += [f"{g:.1f}", "1"]
    L.append("| *one interpreted guest instruction* | " + " | ".join(cells) + " |")
    L += ["", "Units: " + "; ".join(f"{t[0]}: {t[5]}" for t in targets) + ".", ""]

    g7b, rows7b, _ = calc[1]
    L += ["### Against round seven (Unicorn, Cortex-M4 cost table)", "",
          "| Call | Round 7 cycles | ×guest | 7b cycles | ×guest | Change |", "|---|---|---|---|---|---|"]
    for row, (c7, x7) in ROUND7_UNICORN.items():
        c = rows7b[row]
        L.append(f"| {row} | {c7} | {x7} | {c:.0f} | {c / g7b:.1f} | {100 * (c - c7) / c7:+.0f}% |")
    L.append(f"| *one interpreted guest instruction* | {ROUND7_GUEST} | 1 | {g7b:.1f} | 1 | {100 * (g7b - ROUND7_GUEST) / ROUND7_GUEST:+.0f}% |")
    extra = RECURSE_INSTRS["7b"] - RECURSE_INSTRS["round 7"]
    lvl = rows7b["each extra nesting level (test.recurse.v1)"]
    L += ["", f"The nesting row is not like for like: each level of `test.recurse.v1` now runs "
          f"{RECURSE_INSTRS['7b']} guest instructions instead of {RECURSE_INSTRS['round 7']} (it fills a 32-byte frame "
          f"so the shared-stack test has something to measure). Taking out those {extra} instructions "
          f"(×{g7b:.1f}) leaves **{lvl - extra * g7b:.0f} cycles ({lvl / g7b - extra:.1f} ×guest)** of kernel cost per level, "
          f"against 902 (31.1 ×guest) in round seven.", ""]

    L += ["## Async (7d): cost per operation", "",
          "Each figure includes the `ecall` round trip from guest code. *ticket.check*: a guest loop of checks on a "
          "pending ticket, net of the same loop without the op. *timer.start* / *cancel*: one guest call doing 16, net "
          "of an empty guest call, divided by 16. *Select poll* and *wake round trip* are whole kernel operations "
          "(host-to-guest entry included): the first is `poll_task` on a task blocked in `Select2` (two checks); the "
          "second is a GPIO event delivered and processed (ticket done, task woken, polled, a new wait armed).", "",
          "| Operation | " + " | ".join(f"{t[0]} | ×guest" for t in targets) + " |", "|---|" + "---|---|" * len(targets)]
    acs = []
    for t, (g, _, _) in zip(targets, calc):
        try:
            acs.append(async_costs(t[3], g))
        except KeyError:
            acs.append(None)
    for row in next(a for a in acs if a):
        cells = []
        for a, (g, _, _) in zip(acs, calc):
            cells += [f"{a[row]:.0f}", f"{a[row] / g:.1f}"] if a else ["-", "-"]
        L.append(f"| {row} | " + " | ".join(cells) + " |")
    cells = []
    for t, (g, _, _) in zip(targets, calc):
        b = t[3].get("a_budget_burn", {}).get("cycles")
        cells += [f"{b:.0f}" if b else "-", f"{b / g:.0f}" if b else "-"]
    bud = dict(kv.split("=") for kv in targets[0][2].get("async-load", (0, ""))[1].split() if "=" in kv).get("budget", "?")
    L.append(f"| a runaway, cut off by the default budget ({bud} control transfers) | " + " | ".join(cells) + " |")
    L += ["", "Units as in the call-cost table.", ""]

    L += ["## Async (7d): RAM", ""]
    sz = dict(kv.split("=") for kv in targets[1][2].get("async-sizes", (0, ""))[1].split() if "=" in kv)
    hz = dict(kv.split("=") for kv in targets[0][2].get("async-sizes", (0, ""))[1].split() if "=" in kv)
    if sz:
        L += [f"- Ticket table entry: **{sz['ticket_entry']} B** (Thumb) × {sz['tickets']} tickets = {int(sz['ticket_entry']) * int(sz['tickets'])} B.",
              f"- Inbox entry (driver event, up to a 32-byte packet inline): {sz['inbox_entry']} B × {sz['inbox']} = "
              f"{int(sz['inbox_entry']) * int(sz['inbox'])} B; request entry {sz['request_entry']} B × {sz['requests']}; "
              f"the whole shared `Io` (inbox, requests, sent log, counters): **{sz['io_total']} B**.",
              f"- Per applet: the `Applet` record is now {sz['applet_record']} B (spawned/ready task bits, poll entry, fault flag added).",
              f"- Per task slot (guest RAM, in the applet's statics): the future, padded to the slot size, plus 16 B "
              f"(poll and drop function pointers, size). Kernel side per task: 2 bits (spawned, ready).",
              f"- Futures as Rust laid them out (guest, rv32): hello loop {sz['future_hello']} B, receive loop {sz['future_recv']} B "
              f"(holds its 32-byte buffer), ticker {sz['future_ticker']} B, select loop {sz['future_select']} B.",
              f"- x86-64 host for comparison: ticket entry {hz.get('ticket_entry')} B, `Io` {hz.get('io_total')} B.", ""]

    L += ["## Bulk statics (7c): cost per access", "",
          "One read-modify-write (`x += 1`) of a `u32` static in a non-inlined function, called in a loop, "
          "minus the same loop calling an empty function. Guest code per access: gp window "
          "`mv rd,gp` (the rewritten `auipc`) + `lw` + `addi` + `sw`; bulk `lw rd,slot(gp)` + `lw` + `addi` + `sw`. "
          "Same instruction count, same code size; the bulk form turns one register move into a load.", "",
          "| Target | gp-window static | ×guest | bulk static | ×guest | bulk extra |", "|---|---|---|---|---|---|"]
    for t, (g, _, _) in zip(targets, calc):
        a = access_costs(t[3])
        L.append(f"| {t[0]} | {a['gp']:.1f} | {a['gp'] / g:.2f} | {a['bulk']:.1f} | {a['bulk'] / g:.2f} "
                 f"| {a['bulk'] - a['gp']:+.1f} ({(a['bulk'] - a['gp']) / g:+.2f} ×guest) |")
    L += ["", "Units as in the call-cost table. The host figure is wall-clock ns, so its small differences are noise.", ""]
    mf = manifests()
    L += ["## RAM per applet (7c)", "",
          "Flash image = code + rodata (never patched). gp window = small statics plus the bulk slots (4 B each, "
          "one per 4 KB of bulk, +1). Bulk = large and spilled statics. Decode table = 8 B per 2-byte halfword of "
          "image, shared by every instance of the same image.", "",
          "| Applet | Flash image | gp window | of which slots | Bulk | RAM per instance (gp + bulk + record) | Decode table |",
          "|---|---|---|---|---|---|---|"]
    rec = int(targets[1][4].get("applet_bytes", 0))
    gi = os.path.join(HERE, "guest-image")
    for a in APPLETS:
        fl = os.path.getsize(os.path.join(gi, f"{a}.bin"))
        g, b, k = mf.get(a, (0, 0, 0))
        L.append(f"| {a} | {fl} | {g} | {4 * k} | {b} | {g + b + rec} | {fl * 4} |")
    L += ["", f"The `Applet` record is {rec} B on Thumb (import slots, region, gp window and bulk addresses and lengths, gp).", ""]
    tp = os.path.join(HERE, "out", "threshold.md")
    if os.path.exists(tp):
        L += ["## Bulk threshold sweep (7c)", "",
              "`meshops/threshold_sweep.py`: the same applets (and the speed bench's CRC16/parse kernels) linked with "
              "`chunker link-pic --bulk-threshold N`. *none* = threshold off, so only the spill rule moves statics.", ""]
        L += open(tp).read().splitlines() + [""]

    L += ["## Memory and load time", ""]
    u = targets[1][4]
    L += [f"- Registry entry: **{u.get('entry_bytes')} B** on Thumb (8 B hash, 8 B name slice into the applet image, the provider, 4 B reserved hook range, enum/Option padding); "
          f"{targets[0][4].get('entry_bytes')} B on x86-64.",
          f"- Whole registry (32 entries + 64-bucket index): **{u.get('registry_bytes')} B** on Thumb.",
          f"- Per-applet record: **{u.get('applet_bytes')} B** (16 import slots × 2 B, slot count, code region, gp window and bulk addresses and lengths, gp).",
          "- Loading H, A and B (11 imports, 3 exports): "
          + ", ".join(f"{t[0]} {c[2]:.0f} per applet" for t, c in zip(targets, calc)) + ".",
          f"- Host stack per nesting level: **{HOST_STACK_PER_LEVEL} B** on Thumb (`run_guest`'s frame; round seven 464 B). "
          f"Deepest host stack in the whole Unicorn run (includes 7 nested levels): {uj['max_host_stack_bytes']} B.", ""]

    L += ["## Memory model (7b)", ""]
    st = t_info = targets[1][2]
    def detail(name):
        return st.get(name, (False, ""))[1]
    L += [f"- Shared guest stack, Unicorn: `{detail('12-shared-stack')}` (bytes; one 8 KB stack for every applet and level; "
          "round seven gave each level its own 2048 B).",
          "- Shared stack per level on each target: " + ", ".join(
              f"{t[0]} {dict(kv.split('=') for kv in t[2].get('12-shared-stack', (0, ''))[1].split() if '=' in kv).get('per_level', '?')} B"
              for t in targets) + ".",
          f"- Register files: 8 (one per nesting level) × 256 B, owned by the kernel and reused, not cleared, per call.",
          f"- Fit check: `{detail('13-fit')}`.",
          f"- Position independence: `{detail('9-position-independence')}`.",
          f"- Two instances: `{detail('10-instances')}`.",
          f"- Real pointers: `{detail('11-real-pointers')}`.",
          f"- Big buffer (7c): `{detail('14-big-buffer')}`.",
          f"- Spill (7c): `{detail('15-overflow-spill')}`.",
          f"- Bulk position independence (7c): `{detail('16-bulk-position-independence')}`.",
          f"- Bulk real pointer (7c): `{detail('17-bulk-real-pointer')}`.",
          f"- Bulk fit (7c): `{detail('18-bulk-fit')}`.",
          f"- No annotations (7c): `{detail('19-no-annotations')}`.", ""]
    L += ["## ESP32-S3 CRC16 baseline (fixed)", "",
          "The native baseline on the S3 is now the table-driven `kernels::crc16_table`, the same "
          "algorithm the guest image runs (round six used the bitwise loop the Xtensa toolchain emits)."]
    for chip in ("esp32s3", "nrf52840"):
        p = os.path.join(EMU, f"{chip}.uart")
        if os.path.exists(p):
            m = re.search(rf"MVM {chip} crc16 .*interp_cycles=(\d+) native_cycles=(\d+)", open(p, errors="replace").read())
            if m:
                i, n = int(m.group(1)), int(m.group(2))
                L.append(f"- {chip}: interpreted {i}, native {n} → **{i / n:.1f}x**")
    L.append("")
    open(os.path.join(HERE, "RESULTS.md"), "w").write("\n".join(L) + "\n")
    print("\n".join(L))
    return 0 if allpass else 1


if __name__ == "__main__":
    raise SystemExit(main())
