#!/usr/bin/env bash
# Runs the spike's success criteria for both code models.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
C="$ROOT/chunker/target/release/chunker"
OUT="$ROOT/target-check"
LIB="$ROOT/guest/testcrate/src/lib.rs"
HLP="$ROOT/guest/helper/src/lib.rs"
(cd "$ROOT/chunker" && cargo build -q --release)
rm -rf "$OUT" && mkdir -p "$OUT"
cp "$LIB" "$OUT/lib.rs.orig"; cp "$HLP" "$OUT/helper.rs.orig"
restore() { cp "$OUT/lib.rs.orig" "$LIB"; cp "$OUT/helper.rs.orig" "$HLP"; }
trap restore EXIT
pass=0; fail=0
result() { if [ "$1" = ok ]; then echo "  PASS $2"; pass=$((pass+1)); else echo "  FAIL $2"; fail=$((fail+1)); fi; }
build() { rm -rf "$ROOT/guest/target/$1"; CODE_MODEL=$1 "$ROOT/guest/build.sh"; }
changed() { diff <(grep -v '^ ' "$1") <(grep -v '^ ' "$2") | grep '^>' | sed 's/^> [0-9a-f]* //' | tr '\n' ' ' || true; }

for m in small medium; do
  echo "== code model: $m"
  # 1. Every relocation handled, nothing unresolved (the chunker exits non-zero otherwise).
  if "$C" dump $(build $m) > "$OUT/$m.dump" 2>&1 && grep -q "^unresolved: \[\]" "$OUT/$m.dump"; then result ok "all relocations known, no unresolved symbols ($(head -1 "$OUT/$m.dump"))"
  else result fail "chunking: $(tail -1 "$OUT/$m.dump")"; fi

  # 2. Clean rebuild of identical source gives identical hashes.
  "$C" hashes $(build $m) > "$OUT/$m.a"
  "$C" hashes $(build $m) > "$OUT/$m.b"
  if cmp -s "$OUT/$m.a" "$OUT/$m.b"; then result ok "clean rebuild: $(grep -c '^[0-9a-f]' "$OUT/$m.a") named chunks, identical hashes"
  else result fail "clean rebuild changed hashes"; diff "$OUT/$m.a" "$OUT/$m.b" || true; fi

  # 3. Change one function; only its hash may change.
  sed -i 's/helper::mix(t.wrapping_add(x))/helper::mix(t.wrapping_sub(x))/' "$LIB"
  "$C" hashes $(build $m) > "$OUT/$m.edit1"; restore
  got=$(changed "$OUT/$m.a" "$OUT/$m.edit1")
  [ "$got" = "testcrate::combine " ] && result ok "edit combine -> changed: $got" || result fail "edit combine -> changed: $got"

  sed -i 's/x.rotate_left(5) ^ 0x9e37_79b9/x.rotate_left(7) ^ 0x9e37_79b9/' "$HLP"
  "$C" hashes $(build $m) > "$OUT/$m.edit2"; restore
  got=$(changed "$OUT/$m.a" "$OUT/$m.edit2")
  [ "$got" = "helper::mix " ] && result ok "edit helper::mix (other crate) -> changed: $got" || result fail "edit helper::mix -> changed: $got"

  # 4. Functions using the same string point at the same constant chunk.
  h=$(awk '/^[0-9a-f]/{f=$2} /uses .*"hello, mesh"$/{print f, $2}' "$OUT/$m.a")
  echo "$h" | sed 's/^/       /'
  n=$(echo "$h" | awk '{print $2}' | sort -u | wc -l)
  users=$(echo "$h" | wc -l)
  [ "$n" = 1 ] && [ "$users" = 3 ] && result ok "3 functions in 2 crates share one \"hello, mesh\" chunk" || result fail "string sharing"

  # 6. Stretch: rejoin into a flat image and run it.
  if "$C" roundtrip $(build $m) > "$OUT/$m.rt" 2>&1; then result ok "round trip: $(grep -c ' ok ' "$OUT/$m.rt") calls match native"
  else result fail "round trip"; cat "$OUT/$m.rt"; fi
done
echo "== $pass passed, $fail failed"
[ "$fail" = 0 ]
