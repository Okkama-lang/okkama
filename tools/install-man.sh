#!/usr/bin/env bash
# Generate optional section-3 pages from source; used by install and packages.
# Usage: bash tools/install-man.sh HOST_OKKAMA STDLIB PREFIX
set -euo pipefail
GENERATOR="${1:?needs the host okkama binary}"
STDLIB="${2:?needs the std source directory}"
PREFIX="${3:?needs the destination prefix}"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
OKKAMA_STDLIB_DIR="$STDLIB" OKKAMA_IMPORT_DIRS= "$GENERATOR" man --all --format man --output "$STAGE/pages"
mkdir -p "$PREFIX/share/man/man3" "$PREFIX/lib/okkama"
# Generation finishes before touching the installation. Track exactly our own
# filenames so uninstall never removes another package's man pages.
: > "$STAGE/man-pages.txt"
for page in "$STAGE/pages/"*.3ok; do
    install -m 644 "$page" "$PREFIX/share/man/man3/$(basename "$page")"
    basename "$page" >> "$STAGE/man-pages.txt"
done
install -m 644 "$STAGE/man-pages.txt" "$PREFIX/lib/okkama/man-pages.txt"
