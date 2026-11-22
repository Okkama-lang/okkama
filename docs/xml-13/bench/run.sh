# bench/run.sh BIN STDLIB: one series of the six programs (copy bench/micro/support/harness.ok to ../support).
cd "$(dirname "$0")"

B=$1; S=$2
OKKAMA_STDLIB_DIR=$S $B run xml-small.ok 20000 2000 | awk '{printf "xml-small %.2f us %s | ", $4/$2/1000, $5}'
OKKAMA_STDLIB_DIR=$S $B run xml-medium.ok 1000 100 | awk '{printf "xml-medium %.1f us %s | ", $4/$2/1000, $5}'
OKKAMA_STDLIB_DIR=$S $B run xml-large.ok 5 1 | awk '{printf "xml-large %.2f ms %s\n", $4/$2/1e6, $5}'
SIZE=small OKKAMA_STDLIB_DIR=$S $B run plist-sizes.ok 5000 500 | awk '{printf "   plist-small %.2f us %s | ", $4/$2/1000, $5}'
SIZE=medium OKKAMA_STDLIB_DIR=$S $B run plist-sizes.ok 300 30 | awk '{printf "plist-medium %.1f us %s | ", $4/$2/1000, $5}'
SIZE=large OKKAMA_STDLIB_DIR=$S $B run plist-sizes.ok 10 2 | awk '{printf "plist-large %.2f ms %s\n", $4/$2/1e6, $5}'
