# Cortex-M4 cycle cost table

Used by `harness/run.py`. Source: Cortex-M4 TRM r0p1, section 3.3.1 (Table 3-1).
P (pipeline refill after a taken branch) = 2; the TRM range is 1-3.
Zero flash wait states, no interrupts, no bus contention.

| Class | Instructions | Cycles | Note |
|---|---|---|---|
| `alu` | ADD SUB ADC SBC RSB AND ORR ORN EOR BIC MOV MVN LSL LSR ASR ROR RRX CMP CMN TST TEQ UXT* SXT* UBFX SBFX BFI BFC CLZ RBIT REV* MOVW MOVT ADR SEL USAT SSAT ... | 1 | data processing, any operand form |
| `nop` | NOP | 1 |  |
| `it` | IT | 1 | TRM: can fold into the preceding instruction (0); counted as 1 |
| `skipped` | any instruction whose IT/branch condition fails | 1 | executes as a NOP |
| `mul` | MUL MULS SMULL UMULL SMLAL UMLAL UMAAL SMUL* SMLA* (DSP) | 1 | M4 has a single-cycle 32x32 multiplier |
| `mla` | MLA MLS | 2 | multiply-accumulate |
| `div` | SDIV UDIV | 7 | 2-12, data dependent; midpoint used |
| `ld` | LDR LDRB LDRH LDRSB LDRSH LDREX (any addressing mode, incl. literal) | 2 | pipelined model: 1 after another load/store |
| `st` | STR STRB STRH STREX | 2 | pipelined model: 1 after another load/store |
| `ldd` | LDRD STRD | 3 | 1 + 2 |
| `ldm` | LDM POP STM PUSH | 1 + N | N = registers transferred |
| `ldm_pc` | LDM / POP including PC | 1 + N + P |  |
| `ld_pc` | LDR PC, ... | 2 + P |  |
| `b` | B (unconditional) | 1 + P = 3 |  |
| `bcc` | B<cond> | taken 1 + P = 3, untaken 1 |  |
| `cb` | CBZ CBNZ | taken 1 + P = 3, untaken 1 |  |
| `bl` | BL BLX BX | 1 + P = 3 |  |
| `tb` | TBB TBH | 2 + P = 4 | table load + branch |
| `alu_pc` | MOV PC / ADD PC | 1 + P = 3 |  |

Two totals are reported: **table** (loads/stores always 2) and **pipelined**
(a single load/store right after another costs 1, the TRM's best case).
Real hardware lands between them, before flash wait states are added.
