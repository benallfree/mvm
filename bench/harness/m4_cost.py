"""Cortex-M4 cycle cost table used by the Unicorn harness.

Source: Arm Cortex-M4 Technical Reference Manual (r0p1), section 3.3.1,
"Cortex-M4 instruction set summary" (Table 3-1, cycle counts), with the
spike's base rules: 1 default, 2 for loads/stores, 3 for a taken branch,
1 for an untaken one, 2 for multiply-accumulate.

P is the pipeline refill after a taken branch. The TRM gives it as 1-3 cycles
depending on alignment and width of the target; P = 2 makes a taken branch
(1 + P) cost 3, matching the base rule.

Assumes zero flash wait states (code and literal loads always hit), no
interrupts, no bus contention. Run `python3 m4_cost.py` to print this table
as markdown (COSTS.md is that output).
"""

import capstone
import capstone.arm as A

P = 2
DIV = 7  # SDIV/UDIV: 2-12 cycles, data dependent (early termination). Midpoint.

# (class, mnemonics, cost text, note). The order is the order shown in COSTS.md.
TABLE = [
    ("alu", "ADD SUB ADC SBC RSB AND ORR ORN EOR BIC MOV MVN LSL LSR ASR ROR RRX CMP CMN TST TEQ "
            "UXT* SXT* UBFX SBFX BFI BFC CLZ RBIT REV* MOVW MOVT ADR SEL USAT SSAT ...", "1", "data processing, any operand form"),
    ("nop", "NOP", "1", ""),
    ("it", "IT", "1", "TRM: can fold into the preceding instruction (0); counted as 1"),
    ("skipped", "any instruction whose IT/branch condition fails", "1", "executes as a NOP"),
    ("mul", "MUL MULS SMULL UMULL SMLAL UMLAL UMAAL SMUL* SMLA* (DSP)", "1", "M4 has a single-cycle 32x32 multiplier"),
    ("mla", "MLA MLS", "2", "multiply-accumulate"),
    ("div", "SDIV UDIV", f"{DIV}", "2-12, data dependent; midpoint used"),
    ("ld", "LDR LDRB LDRH LDRSB LDRSH LDREX (any addressing mode, incl. literal)", "2", "pipelined model: 1 after another load/store"),
    ("st", "STR STRB STRH STREX", "2", "pipelined model: 1 after another load/store"),
    ("ldd", "LDRD STRD", "3", "1 + 2"),
    ("ldm", "LDM POP STM PUSH", "1 + N", "N = registers transferred"),
    ("ldm_pc", "LDM / POP including PC", "1 + N + P", ""),
    ("ld_pc", "LDR PC, ...", "2 + P", ""),
    ("b", "B (unconditional)", "1 + P = 3", ""),
    ("bcc", "B<cond>", "taken 1 + P = 3, untaken 1", ""),
    ("cb", "CBZ CBNZ", "taken 1 + P = 3, untaken 1", ""),
    ("bl", "BL BLX BX", "1 + P = 3", ""),
    ("tb", "TBB TBH", "2 + P = 4", "table load + branch"),
    ("alu_pc", "MOV PC / ADD PC", "1 + P = 3", ""),
]

MUL1 = {"mul", "muls", "smull", "umull", "smlal", "umlal", "umaal", "smulbb", "smulbt", "smultb",
        "smultt", "smulwb", "smulwt", "smmul", "smmulr", "smlabb", "smlabt", "smlatb", "smlatt",
        "smlawb", "smlawt", "smlad", "smladx", "smlsd", "smlsdx", "smuad", "smusd", "smmla", "smmls"}
MLA = {"mla", "mls"}
DIVS = {"sdiv", "udiv"}
LD = {"ldr", "ldrb", "ldrh", "ldrsb", "ldrsh", "ldrex", "ldrexb", "ldrexh", "ldrt", "ldrbt", "ldrht",
      "ldrsbt", "ldrsht"}
ST = {"str", "strb", "strh", "strex", "strexb", "strexh", "strt", "strbt", "strht"}
LDD = {"ldrd", "strd"}
LDM = {"ldm", "ldmia", "ldmdb", "ldmda", "ldmib", "pop"}
STM = {"stm", "stmia", "stmdb", "stmda", "stmib", "push"}

MD = capstone.Cs(capstone.CS_ARCH_ARM, capstone.CS_MODE_THUMB | capstone.CS_MODE_MCLASS)
MD.detail = True


class Insn:
    """Static facts about one decoded instruction."""

    __slots__ = ("addr", "size", "name", "text", "cc", "cls", "nregs", "writes_pc", "modeled")

    def __init__(self, ci):
        self.addr = ci.address
        self.size = ci.size
        self.name = ci.insn_name()
        self.text = f"{ci.mnemonic} {ci.op_str}"
        self.cc = ci.cc
        _, written = ci.regs_access()
        self.writes_pc = A.ARM_REG_PC in written
        regs = [op for op in ci.operands if op.type == A.ARM_OP_REG]
        self.nregs = len(regs)
        n = self.name
        self.modeled = True
        if n in MUL1:
            self.cls = "mul"
        elif n in MLA:
            self.cls = "mla"
        elif n in DIVS:
            self.cls = "div"
        elif n in LDD:
            self.cls = "ldd"
        elif n in LDM:
            self.cls = "ldm_pc" if self.writes_pc else "ldm"
            if n in ("ldm", "ldmia", "ldmdb", "ldmda", "ldmib"):
                self.nregs -= 1  # base register operand
        elif n in STM:
            self.cls = "ldm"
            if n != "push":
                self.nregs -= 1
        elif n in LD:
            self.cls = "ld_pc" if self.writes_pc else "ld"
        elif n in ST:
            self.cls = "st"
        elif n == "b":
            self.cls = "b" if self.cc in (A.ARM_CC_AL, A.ARM_CC_INVALID) else "bcc"
        elif n in ("cbz", "cbnz"):
            self.cls = "cb"
        elif n in ("bl", "blx", "bx"):
            self.cls = "bl"
        elif n in ("tbb", "tbh"):
            self.cls = "tb"
        elif n == "it":
            self.cls = "it"
        elif n == "nop":
            self.cls = "nop"
        elif self.writes_pc:
            self.cls = "alu_pc"
        else:
            self.cls = "alu"
            # Floating point and system instructions aren't in the table; flag them.
            self.modeled = not (n.startswith("v") or n in ("mrs", "msr", "cpsid", "cpsie", "dmb", "dsb", "isb", "svc", "bkpt", "wfi", "wfe"))

    @property
    def conditional(self):
        return self.cc not in (A.ARM_CC_AL, A.ARM_CC_INVALID)


def cond_passes(cc, cpsr):
    n, z, c, v = (cpsr >> 31) & 1, (cpsr >> 30) & 1, (cpsr >> 29) & 1, (cpsr >> 28) & 1
    return {
        A.ARM_CC_EQ: z == 1, A.ARM_CC_NE: z == 0,
        A.ARM_CC_HS: c == 1, A.ARM_CC_LO: c == 0,
        A.ARM_CC_MI: n == 1, A.ARM_CC_PL: n == 0,
        A.ARM_CC_VS: v == 1, A.ARM_CC_VC: v == 0,
        A.ARM_CC_HI: c == 1 and z == 0, A.ARM_CC_LS: c == 0 or z == 1,
        A.ARM_CC_GE: n == v, A.ARM_CC_LT: n != v,
        A.ARM_CC_GT: z == 0 and n == v, A.ARM_CC_LE: z == 1 or n != v,
    }.get(cc, True)


def cost(i, executed, taken, after_ldst, pipelined):
    """Cycles for one dynamic instance of `i`.

    executed:   its condition passed (always True when unconditional)
    taken:      control actually left the fall-through path
    after_ldst: the previous executed instruction was a single load/store
    pipelined:  apply the TRM's load/store pipelining (N back-to-back -> N+1)
    """
    if not executed:
        return 1
    c = i.cls
    if c in ("alu", "nop", "it", "mul"):
        return 1
    if c == "mla":
        return 2
    if c == "div":
        return DIV
    if c in ("ld", "st"):
        return 1 if (pipelined and after_ldst) else 2
    if c == "ldd":
        return 3
    if c == "ldm":
        return 1 + max(i.nregs, 1)
    if c == "ldm_pc":
        return 1 + max(i.nregs, 1) + P
    if c == "ld_pc":
        return 2 + P
    if c in ("b", "bl", "alu_pc"):
        return 1 + P
    if c in ("bcc", "cb"):
        return 1 + P if taken else 1
    if c == "tb":
        return 2 + P
    return 1


def markdown():
    out = ["# Cortex-M4 cycle cost table", "",
           "Used by `harness/run.py`. Source: Cortex-M4 TRM r0p1, section 3.3.1 (Table 3-1).",
           f"P (pipeline refill after a taken branch) = {P}; the TRM range is 1-3.",
           "Zero flash wait states, no interrupts, no bus contention.", "",
           "| Class | Instructions | Cycles | Note |", "|---|---|---|---|"]
    for cls, ins, cyc, note in TABLE:
        out.append(f"| `{cls}` | {ins} | {cyc} | {note} |")
    out += ["", "Two totals are reported: **table** (loads/stores always 2) and **pipelined**",
            "(a single load/store right after another costs 1, the TRM's best case).",
            "Real hardware lands between them, before flash wait states are added."]
    return "\n".join(out) + "\n"


if __name__ == "__main__":
    print(markdown(), end="")
