"""Shared Unicorn plumbing for the Thumb harnesses: bench/harness/run.py
(rounds 3-5) and meshops/run_unicorn.py (round 7).

- `load_elf`: loadable segments, symbols, and every function decoded from its
  own start (so capstone tracks IT blocks).
- `new_unicorn`: a Cortex-M in Thumb mode with the nRF52840-like map the
  firmware is linked for (flash at 0, RAM at 0x2000_0000), plus a page for the
  return address that ends a call.
- `executed`: whether an instruction's condition passes (IT blocks), read
  before it runs.
- `call`: run one function to completion.
"""
from elftools.elf.elffile import ELFFile
from unicorn import UC_ARCH_ARM, UC_MODE_MCLASS, UC_MODE_THUMB, Uc
from unicorn.arm_const import (UC_ARM_REG_CPSR, UC_ARM_REG_LR, UC_ARM_REG_R0, UC_ARM_REG_R1,
                               UC_ARM_REG_R2, UC_ARM_REG_R3, UC_ARM_REG_SP)

import m4_cost

FLASH = (0x0, 0x10_0000)
RAM = (0x2000_0000, 0x4_0000)
STACK_TOP = RAM[0] + RAM[1]
RET = 0x1000_0000  # return address: execution stops here


def load_elf(path):
    """(segments, symbols, sorted functions, decoded code by address)."""
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
            for ci in m4_cost.MD.disasm(data, a):
                code[ci.address] = m4_cost.Insn(ci)
    return segs, syms, sorted(funcs), code


def new_unicorn(segs):
    uc = Uc(UC_ARCH_ARM, UC_MODE_THUMB | UC_MODE_MCLASS)
    uc.mem_map(*FLASH)
    uc.mem_map(*RAM)
    uc.mem_map(RET, 0x1000)
    for vaddr, data, memsz in segs:
        uc.mem_write(vaddr, data + b"\0" * (memsz - len(data)))
    return uc


def executed(i, uc):
    """False for a conditional (non-branch) instruction whose condition fails."""
    if i is not None and i.conditional and i.cls not in ("bcc", "cb"):
        return m4_cost.cond_passes(i.cc, uc.reg_read(UC_ARM_REG_CPSR))
    return True


def call(uc, addr, args=(), count=50_000_000):
    """Call the Thumb function at `addr` with up to 4 arguments; return r0."""
    uc.reg_write(UC_ARM_REG_SP, STACK_TOP)
    uc.reg_write(UC_ARM_REG_LR, RET | 1)
    for r, v in zip((UC_ARM_REG_R0, UC_ARM_REG_R1, UC_ARM_REG_R2, UC_ARM_REG_R3), args):
        uc.reg_write(r, v)
    uc.emu_start(addr | 1, RET, count=count)
    return uc.reg_read(UC_ARM_REG_R0)
