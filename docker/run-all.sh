#!/usr/bin/env bash
# Every spike check, in order: chunker (check.sh), the Unicorn speed bench
# (bench/run.sh), the emulator runs (emu/run.sh), and the round-seven mesh ops
# (meshops/run.sh: x86 host + Unicorn, plus the emulator results emu/run.sh produced).
set -uo pipefail
cd "$(dirname "$0")/.."
declare -A rc
for s in check.sh bench/run.sh emu/run.sh meshops/run.sh; do
  echo "######## $s"
  ./$s; rc[$s]=$?
done
echo
echo "######## summary"
fail=0
for s in check.sh bench/run.sh emu/run.sh meshops/run.sh; do
  printf '%-14s %s\n' "$s" "$([ ${rc[$s]} = 0 ] && echo PASS || echo "FAIL (exit ${rc[$s]})")"
  [ ${rc[$s]} = 0 ] || fail=1
done
exit $fail
