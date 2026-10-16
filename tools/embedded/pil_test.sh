#!/usr/bin/env bash
# Model / software / processor in the loop for the motor speed loop
# (Daedalus.Examples.Embedded): the discrete PID is generated as C and run
#   - on the host (software in the loop),
#   - on an emulated Arduino Uno, ATmega328P at 16 MHz in simavr,
#   - on an emulated ESP32 in Espressif's QEMU,
# each in closed loop with MotorBench simulated by DAEdalus, and every run is
# compared with the all-Modelica loop (ClosedLoop). The continuous PI (SpeedPI)
# runs on the host and the Uno too, with RK4 at 1 ms. The "arduino" target builds the
# generated Arduino sketch with arduino-cli (Arduino core, Serial, micros()) for the Uno
# and runs it in simavr too; with the esp32:esp32 core installed it also checks that the
# sketch builds for an ESP32 board.
#
# usage: tools/embedded/pil_test.sh [host] [avr] [esp32] [arduino]   (default: host avr esp32)
# env:   DAEDALUS (CLI binary), OUT (work directory), ARDUINO_CLI
set -euo pipefail
cd "$(dirname "$0")/../.."
D=${DAEDALUS:-$PWD/target/release/daedalus}
OUT=${OUT:-/tmp/daedalus-pil}
LIB=$PWD/library/Daedalus
TARGETS=${*:-host avr esp32}
CMP="python3 $PWD/tools/embedded/compare.py"
mkdir -p "$OUT"
SUMMARY="$OUT/summary.md"
echo "| Controller | Target | Max rel. error u | Max rel. error y | Cycles per step |" > "$SUMMARY"
echo "| --- | --- | --- | --- | --- |" >> "$SUMMARY"

row() { # name target csv mil uvar yvar tol
  local res
  res=$($CMP "$4" "$3" --map "u=$5" --map "y=$6" --tol "$7" --rel) || { echo "$res"; echo "FAILED: $1 on $2"; exit 1; }
  echo "$res"
  local eu ey cyc
  eu=$(echo "$res" | awk '/^u vs/ {print $7}')
  ey=$(echo "$res" | awk '/^y vs/ {print $7}')
  cyc=$(echo "$res" | awk '/controller step/ {print $4 " mean, " $7 " max"}')
  case $2 in *QEMU*) cyc="${cyc:+$cyc (QEMU, not cycle-accurate)}" ;; esac
  echo "| $1 | $2 | $eu | $ey | ${cyc:-n/a} |" >> "$SUMMARY"
}

echo "== model in the loop"
"$D" simulate "$LIB" -m Daedalus.Examples.Embedded.ClosedLoop -o "$OUT/mil_pid.csv" --vars bench.y,pid.u >/dev/null
"$D" simulate "$LIB" -m Daedalus.Examples.Embedded.ClosedLoopPI -o "$OUT/mil_pi.csv" --vars bench.y,pi.u >/dev/null

echo "== code generation"
"$D" codegen-c "$LIB" -m Daedalus.Examples.Embedded.DiscretePID --step 0.01 --prefix pid --hil -o "$OUT/pid"
"$D" codegen-c "$LIB" -m Daedalus.Examples.Embedded.SpeedPI --step 0.001 --prefix pi --hil -o "$OUT/pi"
"$D" codegen-c "$LIB" -m Daedalus.Examples.Embedded.DiscretePID --step 0.01 --prefix pidd --real double --hil -o "$OUT/pidd"

hil() { # controller-dir prefix out.csv exec...
  local dir=$1 p=$2 csv=$3
  shift 3
  "$D" hil "$LIB" -m Daedalus.Examples.Embedded.MotorBench --controller "$dir/$p.json" \
    --stop 3 -o "$csv" --reply-timeout 60 --exec "$*"
}

for t in $TARGETS; do
  case $t in
  host)
    echo "== software in the loop (host)"
    for p in pid pi pidd; do make -s -C "$OUT/$p/hil/host" CFLAGS="-O2 -std=c99 -Wall -Wextra -Werror"; done
    hil "$OUT/pid" pid "$OUT/sil_pid.csv" "$OUT/pid/hil/host/pid_sil"
    row "discrete PID, float" host "$OUT/sil_pid.csv" "$OUT/mil_pid.csv" pid.u bench.y 1e-5
    hil "$OUT/pidd" pidd "$OUT/sil_pidd.csv" "$OUT/pidd/hil/host/pidd_sil"
    row "discrete PID, double" host "$OUT/sil_pidd.csv" "$OUT/mil_pid.csv" pid.u bench.y 1e-6
    hil "$OUT/pi" pi "$OUT/sil_pi.csv" "$OUT/pi/hil/host/pi_sil"
    row "continuous PI, RK4 1 ms" host "$OUT/sil_pi.csv" "$OUT/mil_pi.csv" pi.u bench.y 1e-2
    ;;
  avr)
    echo "== processor in the loop: Arduino Uno (ATmega328P, simavr)"
    cc -O2 -o "$OUT/simavr_bridge" tools/embedded/simavr_bridge.c -lsimavr -lelf
    make -s -C "$OUT/pid/hil/avr"
    make -s -C "$OUT/pi/hil/avr"
    hil "$OUT/pid" pid "$OUT/avr_pid.csv" "$OUT/simavr_bridge" "$OUT/pid/hil/avr/pid.elf"
    row "discrete PID, float" "ATmega328P (simavr)" "$OUT/avr_pid.csv" "$OUT/mil_pid.csv" pid.u bench.y 1e-5
    hil "$OUT/pi" pi "$OUT/avr_pi.csv" "$OUT/simavr_bridge" "$OUT/pi/hil/avr/pi.elf"
    row "continuous PI, RK4 1 ms" "ATmega328P (simavr)" "$OUT/avr_pi.csv" "$OUT/mil_pi.csv" pi.u bench.y 1e-2
    ;;
  esp32)
    echo "== processor in the loop: ESP32 (QEMU)"
    make -s -C "$OUT/pid/hil/esp32"
    make -s -C "$OUT/pidd/hil/esp32"
    for p in pid pidd; do
      hil "$OUT/$p" $p "$OUT/esp32_$p.csv" qemu-system-xtensa -machine esp32 -nographic -monitor none \
        -serial stdio -drive "file=$OUT/$p/hil/esp32/${p}_flash.bin,if=mtd,format=raw" -nic none
    done
    row "discrete PID, float" "ESP32 (QEMU)" "$OUT/esp32_pid.csv" "$OUT/mil_pid.csv" pid.u bench.y 1e-5
    row "discrete PID, double" "ESP32 (QEMU)" "$OUT/esp32_pidd.csv" "$OUT/mil_pid.csv" pid.u bench.y 1e-6
    ;;
  arduino)
    echo "== processor in the loop: Arduino sketch on an Uno (arduino-cli, simavr)"
    AC=${ARDUINO_CLI:-arduino-cli}
    [ -x "$OUT/simavr_bridge" ] || cc -O2 -o "$OUT/simavr_bridge" tools/embedded/simavr_bridge.c -lsimavr -lelf
    "$AC" compile --fqbn arduino:avr:uno --output-dir "$OUT/pid/ard_uno" "$OUT/pid/hil/arduino/pid_hil"
    hil "$OUT/pid" pid "$OUT/ard_uno_pid.csv" "$OUT/simavr_bridge" "$OUT/pid/ard_uno/pid_hil.ino.elf"
    row "discrete PID, float, Arduino core" "Uno sketch (simavr)" "$OUT/ard_uno_pid.csv" "$OUT/mil_pid.csv" pid.u bench.y 1e-5
    if "$AC" core list 2>/dev/null | grep -q '^esp32:esp32'; then
      "$AC" compile --fqbn esp32:esp32:esp32 --output-dir "$OUT/pid/ard_esp32" "$OUT/pid/hil/arduino/pid_hil"
      echo "| discrete PID, float, Arduino core | ESP32 sketch | builds (run it on a board) | | |" >> "$SUMMARY"
    fi
    ;;
  *) echo "unknown target $t"; exit 2 ;;
  esac
done
echo
cat "$SUMMARY"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  { echo "### Embedded code in the loop (motor speed control)"; echo; cat "$SUMMARY"; } >> "$GITHUB_STEP_SUMMARY"
fi
