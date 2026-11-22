# run.sh BIN STDLIB: one series (format and parse, three sizes).
cd "$(dirname "$0")"  # copy bench/micro/support/harness.ok to ../support
B=$1; S=$2
for kind in format parse; do
  for size in small medium large; do
    KIND=$kind SIZE=$size OKKAMA_STDLIB_DIR=$S $B run time-format.ok 20000 2000 | awk -v k=$kind -v s=$size '{printf "%s-%s %.2f us %s | ", k, s, $4/$2/1000, $5}'
  done
  echo
done
