# MVM

MVM is the Mesh Virtual Machine. Read these before working on it:

- [docs/mesh_virtual_machine_5ae0e741.plan.md](docs/mesh_virtual_machine_5ae0e741.plan.md), the design plan: what each crate is, and the locked decisions.
- [docs/implementation-plan.md](docs/implementation-plan.md), the implementation plan: the order of steps, each with Do, Check, and Expect, plus the drift guards and the results log. Its "How to use this file" section still applies, and this file adds to it.
- [docs/icebox.md](docs/icebox.md): ideas deferred past v1. A new idea that is not v1 goes here, not into the plans.

v1 trusts the operator and every applet. Don't add grants, isolation, signing, or misuse hardening. If one seems necessary, ask first.

## How a step gets built

Build one implementation-plan step at a time, in the order below. Each phase ends at a gate. Stop at every gate and wait for the user's go-ahead. Never start the next phase or the next step on your own.

### 1. Prepare

Before writing any code for step N.M:

- Read the step, every design-plan section it names or depends on, the `Result:` lines of the earlier steps it builds on, and any open question it is meant to answer.
- Write down what is missing or unclear, for example:
  - a name, signature, byte layout, or config key that neither plan pins down;
  - a Check that doesn't exercise everything the Do builds;
  - an Expect that can't be measured or can't fail;
  - a dependency on something no earlier step builds;
  - board cells that can't run yet (a planned board, missing hardware), and why.
- Propose an answer for each gap. Prefer the simplest answer that fits the design.
- Run the ripple check (below) on what you propose.

**Gate:** Present the gaps, your proposals, and the ripple. The user decides. Then edit the plans to match what was decided, before writing code, so the step's text says what will be built.

### 2. Build

- Build only what the step's Do says, as refined in Prepare. When building turns up something Prepare missed, stop and raise it. Don't make a design choice quietly in code.
- Write the tests the Check needs as part of the step, not afterward.
- Match the plans' names exactly: crates, meshops, shell commands, config keys, and error codes.

### 3. Test

- Run the step's Check on everything it names: the host, the simulator (`SIM`), and each bench board.
- Run the regression set too: `cargo fmt --check`, `cargo clippy` with warnings as errors, host tests, and every drift-guard test that exists so far.
- You can't touch the boards. When a Check needs a board you can't reach, give the user the exact command to run (normally `cargo xtask bench`) and wait for the output. Never mark a cell as passed without seeing that output.

### 4. Verify

Compare what happened with the Expect, item by item. Then check:

- Every cell of the step's matrix is `✅`, `❌` with a note, `➖` with a reason, or `⬜` for a planned board that isn't here yet.
- The drift guards still hold. Changes to the loader, `mvm-mem`, or `mvm-code` are also reviewed for fixed-size guest buffers, per-applet arenas, and image copies.
- The code matches the plans as edited in Prepare.

**Gate:** Report the results. Say what passed, what failed with its output, what wasn't run and why, and anything surprising: measurements, sizes, timings, or answers to open questions. Discuss them with the user.

### 5. Ripple check and record

Run the ripple check again against what the step actually produced and what the discussion decided. Then record:

- Check the step's box only if its Check passed. A step with a failing cell stays unchecked.
- Add the step's `Result:` line: the date, what you saw, and what changed in the design.
- Fill in the matrix cells.
- Make the design-plan and later-step edits the ripple check found.
- Remove any open question the step answered, with the answer in the `Result:` line.
- When a decision reaches past this step, add a dated line to the implementation plan's results log.

**Gate:** Show the user the diff to the docs, and a commit message. Commit only on their go-ahead.

### 6. Commit

One commit per step. It holds the code, the tests, the box and `Result:` line, and every plan edit the step caused. The subject names the step, for example `0.2 Create the Cargo workspace`.

## The ripple check

Run it in Prepare and again after Verify. For each name, decision, or measured number that the step adds or changes:

- **Design plan.** Does it contradict a locked decision, or change one? A changed locked decision or drift guard needs the user's explicit sign-off.
- **Later steps.** Search the implementation plan for the name and for the area it touches. Fix every Do, Check, Expect, and matrix that now reads wrong. Add, split, move, or drop steps as needed, following the plan's rules on step ids and dropped steps.
- **Earlier steps.** Does a finished step's `Result:` line or tested behavior no longer hold? Say so. Never rewrite a finished step's result.
- **Open questions and the icebox.** Did a question get answered, or a new one come up? Does an icebox entry become needed for v1 (ask first), or does part of this step belong in the icebox?
- **Numbers.** When a measured size, speed, or RAM figure differs from what a later step assumes (page counts, cache sizes, `instr_per_tick`), find every place that depends on it.

Report what the check found, even when it found nothing, so the user knows it ran: "Ripple: none" or a short list of each place and the edit.

## Reviews

When reviewing a plan or a step, look for logical gaps and missing features: things that can't work as written, steps that contradict each other, and features nothing builds or reaches. Don't tighten error codes, add refusals, or defend against misuse.
