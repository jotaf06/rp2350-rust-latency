#!/usr/bin/env bash
# Build the Zephyr latency firmware (zephyr-latency/) and flash it over USB.
#
#   tools/zephyr.sh <preempt|coop> [release|release-z] [extra cmake args...]
#   tools/zephyr.sh preempt
#   CHIP=rp2040 tools/zephyr.sh coop release-z
#   NOFLASH=1 tools/zephyr.sh preempt          (build only)
#
# CHIP: rp2350 (default; board rpi_pico2/rp2350a/m33, also runs on RP2350B)
#       or rp2040 (board rpi_pico).
# ZEPHYR_WS: west workspace with Zephyr used to build (only read, never
#       modified); default ~/zephyrproject. It must have a Python venv with
#       west in $ZEPHYR_WS/.venv (see README, "Requisito: workspace do Zephyr").
#
# Flashing works like tools/flash.sh: a running measurement firmware on PORT
# (default /dev/ttyACM0) is rebooted into BOOTSEL with the 1200-baud touch,
# then picotool loads the ELF. Every build is pristine (-p always), so cached
# CMake options from an earlier build (e.g. -DEXTRA_CFLAGS=-DLAT_DEBUG) never leak.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

MODE="${1:?usage: tools/zephyr.sh <preempt|coop> [release|release-z] [extra cmake args...]}"
PROFILE="${2:-release}"
shift; [ $# -gt 0 ] && shift
CHIP="${CHIP:-rp2350}"
WS="${ZEPHYR_WS:-$HOME/zephyrproject}"
if [ ! -f "$WS/.venv/bin/activate" ] || [ ! -d "$WS/zephyr" ]; then
    echo "no Zephyr west workspace at $WS (set ZEPHYR_WS; see README)" >&2
    exit 1
fi
PORT="${PORT:-/dev/ttyACM0}"

case "$CHIP" in
    rp2350) BOARD=rpi_pico2/rp2350a/m33 ;;
    rp2040) BOARD=rpi_pico ;;
    *) echo "CHIP must be rp2350 or rp2040 (got '$CHIP')" >&2; exit 1 ;;
esac
case "$MODE" in preempt|coop) ;; *) echo "mode must be preempt or coop" >&2; exit 1 ;; esac
case "$PROFILE" in release|release-z) ;; *) echo "profile must be release or release-z" >&2; exit 1 ;; esac

CONF="prj.conf;prof-$PROFILE.conf"
[ "$MODE" = coop ] && CONF="$CONF;mode-coop.conf"
BUILD="$ROOT/build-zephyr/${CHIP}_${MODE}_${PROFILE}"

# west has to run inside its workspace; the app and build dir stay here.
# shellcheck disable=SC1091
source "$WS/.venv/bin/activate"
export ZEPHYR_BASE="$WS/zephyr"
(cd "$WS" && west build -p always -b "$BOARD" -d "$BUILD" "$ROOT/zephyr-latency" -- \
    -DCONF_FILE="$CONF" "$@")

[ -n "${NOFLASH:-}" ] && exit 0

if [ -c "$PORT" ]; then
    echo "asking the running firmware on $PORT to reboot into BOOTSEL..."
    stty -F "$PORT" 1200 2>/dev/null || true
    sleep 3
fi
picotool load -u -v -x -t elf "$BUILD/zephyr/zephyr.elf"
