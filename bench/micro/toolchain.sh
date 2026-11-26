#!/usr/bin/env bash
# toolchain.sh WORKTREE OUT [LAYOUT_SEED]
# A micro A/B toolchain built as the packages are (packages.yml): fat LTO release, --cfg okkama_release (no
# devtrace, #1025), frame pointers, in the okkama-dev image, into OUT/bin/okkama with OUT/lib/okkama/std.
# LAYOUT_SEED (a positive integer) links with rust-lld and --shuffle-sections=*=SEED: the same code in another
# function order, so two seeds of one revision show how much of a cell is the binary's own layout (#1086).
set -euo pipefail
worktree=$(cd "$1" && pwd); out=$2; seed=${3:-}
name=$(basename "$worktree")
flags="--cfg okkama_release -C force-frame-pointers=yes"
if [ -n "$seed" ]; then
  case $seed in ''|*[!0-9]*|0) echo "LAYOUT_SEED must be a positive integer, got '$seed'" >&2; exit 2;; esac
  # gcc-ld holds the ld.lld wrapper rustc ships; seed 0 would mean a random order, so it is refused above.
  flags="$flags -C link-arg=-fuse-ld=lld -C link-arg=-B@SYSROOT@/lib/rustlib/@TARGET@/bin/gcc-ld -C link-arg=-Wl,--shuffle-sections=*=$seed"
fi
mkdir -p "$out/bin" "$out/lib/okkama"
docker run --rm --cpus 6 --memory 7g --network none \
  -v "$worktree":/repo:ro -v "okkama-gate-target-$name${seed:+-seed$seed}":/target -v okkama-cargo-registry:/usr/local/cargo/registry \
  -v "$(cd "$out" && pwd)":/out -e CARGO_TARGET_DIR=/target -e CARGO_NET_OFFLINE=true -e FLAGS="$flags" \
  -w /repo/okkama-rs okkama-dev sh -c '
    export RUSTFLAGS=$(echo "$FLAGS" | sed -e "s|@SYSROOT@|$(rustc --print sysroot)|" -e "s|@TARGET@|$(rustc -vV | sed -n "s/^host: //p")|")
    cargo build --release --locked --offline -p okkama-cli -p sparrow-cli -p blade-cli && cp /target/release/okkama /out/bin/okkama'
rm -rf "$out/lib/okkama/std"
cp -R "$worktree/std" "$out/lib/okkama/std"
if grep -q -a OKKAMA_DEV_OUT "$out/bin/okkama"; then echo "devtrace is compiled in: not a release build" >&2; exit 1; fi
shasum -a 256 "$out/bin/okkama"
