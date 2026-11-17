#!/usr/bin/env bash
# Memory footprint of the blink binaries, for Table C (Table 4 of the paper).
#
#   tools/sizes.sh [output-dir]        (default: results/)
#   CHIP=rp2040 tools/sizes.sh         (Raspberry Pi Pico; default CHIP=rp2350)
#
# Builds blink (and rtic_blink, if present) in both profiles and writes the
# Berkeley `size` columns (text, data, bss) to <output-dir>/sizes.csv. Rows of
# the other chip already in the file are kept, so both chips can be compared.
# tools/analyze.py picks that file up for summary.md.
set -euo pipefail
cd "$(dirname "$0")/.."

OUT="${1:-results}"
CHIP="${CHIP:-rp2350}"
case "$CHIP" in
    rp2350) TARGET=thumbv8m.main-none-eabihf; CHIP_ARGS=() ;;
    rp2040) TARGET=thumbv6m-none-eabi
            CHIP_ARGS=(--no-default-features --features rp2040 --target "$TARGET") ;;
    *) echo "CHIP must be rp2350 or rp2040 (got '$CHIP')" >&2; exit 1 ;;
esac
mkdir -p "$OUT"

# Any `size` will do: arm-none-eabi-size, llvm-size on PATH, or the one that
# comes with `rustup component add llvm-tools`.
if command -v arm-none-eabi-size >/dev/null; then
    SIZE=arm-none-eabi-size
elif command -v llvm-size >/dev/null; then
    SIZE=llvm-size
else
    SIZE="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin/llvm-size"
    if [ ! -x "$SIZE" ]; then
        echo "no size tool found: run 'rustup component add llvm-tools' (or install arm-none-eabi-binutils)" >&2
        exit 1
    fi
fi

BINS=(blink)
[ -f src/bin/rtic_blink.rs ] && BINS+=(rtic_blink)

CSV="$OUT/sizes.csv"
KEEP=""
if [ -f "$CSV" ] && head -n 1 "$CSV" | grep -q '^chip,'; then
    KEEP="$(tail -n +2 "$CSV" | grep -v "^$CHIP," || true)"
fi
echo "chip,bin,profile,opt,text,data,bss,total" > "$CSV"
[ -n "$KEEP" ] && echo "$KEEP" >> "$CSV"
for profile in release release-z; do
    opt=3; [ "$profile" = release-z ] && opt=z
    for bin in "${BINS[@]}"; do
        cargo build --quiet --profile "$profile" --bin "$bin" "${CHIP_ARGS[@]}"
        read -r text data bss _ < <("$SIZE" "target/$TARGET/$profile/$bin" | tail -n 1)
        echo "$CHIP,$bin,$profile,$opt,$text,$data,$bss,$((text + data + bss))" >> "$CSV"
    done
done

echo "wrote $OUT/sizes.csv"
column -s, -t "$OUT/sizes.csv"
