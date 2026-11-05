#!/usr/bin/env bash
# Runs the spike's checks. Round one: criteria 1-6 for both code models.
# Round two: version bumps, a different checkout path, and the core-using applet.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
C="$ROOT/chunker/target/release/chunker"
OUT="$ROOT/target-check"
G="$ROOT/guest"
LIB="$G/testcrate/src/lib.rs"
HLP="$G/helper/src/lib.rs"
HTOML="$G/helper/Cargo.toml"
(cd "$ROOT/chunker" && cargo build -q --release)
rm -rf "$OUT" && mkdir -p "$OUT/orig"
for f in "$LIB" "$HLP" "$HTOML" "$G/Cargo.lock"; do cp "$f" "$OUT/orig/$(echo "$f" | tr / _)"; done
restore() { for f in "$LIB" "$HLP" "$HTOML" "$G/Cargo.lock"; do cp "$OUT/orig/$(echo "$f" | tr / _)" "$f"; done; }
trap restore EXIT
pass=0; fail=0
result() { if [ "$1" = ok ]; then echo "  PASS $2"; pass=$((pass+1)); else echo "  FAIL $2"; fail=$((fail+1)); fi; }
# build <code model> [guest dir] [extra env...]: clean build, prints chunker inputs.
build() { local m=$1 g=${2:-$G}; rm -rf "$g/target/testcrate-$m-ltooff"; CODE_MODEL=$m "$g/build.sh"; }
# Names whose manifest line (name -> hash) differs between two `hashes` outputs.
changed() { diff <(grep -v '^ ' "$1") <(grep -v '^ ' "$2") | grep '^>' | sed 's/^> [0-9a-f]* //' | tr '\n' ' ' || true; }
# The raw crate-hash prefix rustc gave helper in a build (to show the raw names really moved).
rawhelper() { echo "$1" | tr ' ' '\n' | grep libhelper | xargs llvm-nm 2>/dev/null | grep -o 'Cs[0-9A-Za-z]*_6helper' | sort -u | head -1; }
set_version() { sed -i "s/^version = \".*\"/version = \"$1\"/" "$HTOML"; }

# A second checkout of the guest sources at a different path.
OTHER="$OUT/elsewhere/checkout-two"
mkdir -p "$OTHER" && (cd "$G" && tar cf - --exclude=./target .) | (cd "$OTHER" && tar xf -)

for m in small medium; do
  echo "== testcrate, code model: $m"
  # 1. Every relocation handled, nothing unresolved.
  if "$C" dump $(build $m) > "$OUT/$m.dump" 2>&1 && grep -q "^unresolved: \[\]" "$OUT/$m.dump"; then result ok "all relocations known, no unresolved symbols ($(head -1 "$OUT/$m.dump"))"
  else result fail "chunking: $(tail -1 "$OUT/$m.dump")"; fi

  # 2. Clean rebuild of identical source gives identical hashes.
  IN=$(build $m); "$C" hashes $IN > "$OUT/$m.a"; RAW0=$(rawhelper "$IN")
  "$C" hashes $(build $m) > "$OUT/$m.b"
  if cmp -s "$OUT/$m.a" "$OUT/$m.b"; then result ok "clean rebuild: $(grep -c '^[0-9a-f]' "$OUT/$m.a") manifest entries, identical names and hashes"
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
  n=$(echo "$h" | awk '{print $2}' | sort -u | wc -l); users=$(echo "$h" | wc -l)
  [ "$n" = 1 ] && [ "$users" = 3 ] && result ok "3 functions in 2 crates share one \"hello, mesh\" chunk" || result fail "string sharing"

  # 6. Rejoin into a flat image and run it.
  if "$C" roundtrip $(build $m) > "$OUT/$m.rt" 2>&1; then result ok "round trip: $(grep -c ' ok ' "$OUT/$m.rt") calls match native"
  else result fail "round trip"; cat "$OUT/$m.rt"; fi

  # 7. Version bump, no code change: every name and hash identical.
  set_version 0.1.1; IN=$(build $m); "$C" hashes $IN > "$OUT/$m.v011"; RAW1=$(rawhelper "$IN"); restore
  if cmp -s "$OUT/$m.a" "$OUT/$m.v011" && [ "$RAW0" != "$RAW1" ]; then result ok "helper 0.1.0 -> 0.1.1: raw crate hash $RAW0 -> $RAW1, all names and hashes identical"
  else result fail "helper 0.1.1 (raw $RAW0 -> $RAW1)"; diff "$OUT/$m.a" "$OUT/$m.v011" || true; fi

  # 8. Body change + bump: only that function's chunk and manifest entry change.
  set_version 0.1.2; sed -i 's/x.rotate_left(5) ^ 0x9e37_79b9/x.rotate_left(7) ^ 0x9e37_79b9/' "$HLP"
  "$C" hashes $(build $m) > "$OUT/$m.v012"; restore
  got=$(changed "$OUT/$m.a" "$OUT/$m.v012")
  lines=$(diff "$OUT/$m.a" "$OUT/$m.v012" | grep -c '^>' || true)
  [ "$got" = "helper::mix " ] && [ "$lines" = 1 ] && result ok "helper 0.1.2 + mix body change -> only manifest line changed: $got" || result fail "helper 0.1.2 -> changed: $got ($lines lines)"

  # 9. Different checkout path: identical.
  IN=$(build $m "$OTHER"); "$C" hashes $IN > "$OUT/$m.path"; RAW2=$(rawhelper "$IN")
  # (Cargo hashes workspace members by workspace-relative path, so the raw crate hash may not move here.)
  [ "$RAW0" = "$RAW2" ] && note="raw crate hash unchanged" || note="raw crate hash $RAW0 -> $RAW2"
  if cmp -s "$OUT/$m.a" "$OUT/$m.path"; then result ok "other checkout path ($note): all names and hashes identical"
  else result fail "other checkout path (raw $RAW0 -> $RAW2)"; diff "$OUT/$m.a" "$OUT/$m.path" || true; fi
done

echo "== applet (core formatting, memcpy, bounds-check panic; core rebuilt with -Zbuild-std)"
abuild() { rm -rf "$1/target/applet-small-std-ltooff"; PKG=applet BUILD_STD=1 "$1/build.sh"; }
AIN=$(abuild "$G"); TIN=$(build small)
if "$C" names $AIN $TIN > "$OUT/names" 2>&1; then result ok "names over core + compiler_builtins + our crates: $(grep total "$OUT/names" | awk '{print $2+$3}') names, 0 stable / 0 8-byte / 0 16-byte collisions"
else result fail "names: $(tail -2 "$OUT/names")"; fi
if "$C" applet-roundtrip $AIN > "$OUT/applet.rt" 2>&1; then result ok "applet round trip: $(grep -c ': ok' "$OUT/applet.rt") of 5 cases (3 match native, 2 panic into the handler)"
else result fail "applet round trip"; cat "$OUT/applet.rt"; fi
"$C" hashes $AIN > "$OUT/applet.a"
"$C" hashes $(abuild "$OTHER") > "$OUT/applet.path"
if cmp -s "$OUT/applet.a" "$OUT/applet.path"; then result ok "applet from other checkout path: $(grep -c '^[0-9a-f]' "$OUT/applet.a") manifest entries (incl. all of core), identical"
else result fail "applet other path: $(diff "$OUT/applet.a" "$OUT/applet.path" | grep -c '^>') lines differ"; fi

echo "== $pass passed, $fail failed"
[ "$fail" = 0 ]
