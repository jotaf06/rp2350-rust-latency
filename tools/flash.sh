#!/usr/bin/env bash
# Build one binary and flash it over USB.
#
#   tools/flash.sh <bin> [profile] [extra cargo args...]
#   tools/flash.sh s1_thread_load
#   tools/flash.sh s0_thread_idle release-z
#   tools/flash.sh s0_thread_idle release --features clk-64mhz
#   CHIP=rp2040 tools/flash.sh s0_thread_idle      (Raspberry Pi Pico, RP2040)
#
# CHIP selects the board: rp2350 (Pico 2, default) or rp2040 (Pico).
#
# If a measurement firmware is already running, it is asked to reboot into the
# BOOTSEL bootloader by opening its serial port at 1200 baud (PORT=/dev/ttyACM0
# by default). Otherwise hold BOOTSEL while plugging the board in.
#
# Flashing uses picotool. Without picotool, a UF2 file is written instead and,
# if the RP2350 drive is mounted, copied to it.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="${1:?usage: tools/flash.sh <bin> [profile] [extra cargo args...]}"
PROFILE="${2:-release}"
shift; [ $# -gt 0 ] && shift
PORT="${PORT:-/dev/ttyACM0}"
CHIP="${CHIP:-rp2350}"
case "$CHIP" in
    rp2350) TARGET=thumbv8m.main-none-eabihf; CHIP_ARGS=(); DRIVE_LABEL=RP2350 ;;
    rp2040) TARGET=thumbv6m-none-eabi; DRIVE_LABEL=RPI-RP2
            CHIP_ARGS=(--no-default-features --features rp2040 --target "$TARGET") ;;
    *) echo "CHIP must be rp2350 or rp2040 (got '$CHIP')" >&2; exit 1 ;;
esac
ELF="target/$TARGET/$PROFILE/$BIN"

cargo build --profile "$PROFILE" --bin "$BIN" "${CHIP_ARGS[@]}" "$@"

if [ -c "$PORT" ]; then
    echo "asking the running firmware on $PORT to reboot into BOOTSEL..."
    stty -F "$PORT" 1200 2>/dev/null || true
    sleep 3
fi

if command -v picotool >/dev/null; then
    picotool load -u -v -x -t elf "$ELF"
else
    echo "picotool not found; falling back to a UF2 file" >&2
    python3 tools/elf2uf2.py "$ELF" "$ELF.uf2" "$CHIP"
    DRIVE="$(findmnt -rn -o TARGET -S LABEL=$DRIVE_LABEL 2>/dev/null | head -n 1 || true)"
    if [ -n "$DRIVE" ]; then
        cp "$ELF.uf2" "$DRIVE/" && sync
        echo "copied to $DRIVE; the board reboots into the new firmware"
    else
        echo "$DRIVE_LABEL drive not mounted: copy $ELF.uf2 to it by hand" >&2
    fi
fi
