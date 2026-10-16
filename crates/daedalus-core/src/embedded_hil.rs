//! Closed-loop tests of generated embedded code: software in the loop (the generated C
//! compiled for the host) and processor in the loop (cross-compiled for a
//! microcontroller, running on an emulator or a real board). DAEdalus simulates the
//! plant (a "bench" model with top-level inputs for the controller's outputs) and
//! exchanges one frame per sample with the controller over a byte stream.
//!
//! Wire protocol (all numbers little-endian; `R` is the controller's `real_t`, 4 or 8 bytes):
//!
//! - the firmware prints `DAEDALUS-HIL <prefix> <n_in> <n_out> <bytes of R>\n` when it is
//!   ready (anything before that line, such as a boot ROM's messages, is ignored);
//! - the host sends `0xA5 'I'` (start) or `0xA5 'S'` (step) followed by `n_in` values `R`;
//! - the firmware answers `0x5A status` then `n_out` values `R` and a `u32` with the
//!   processor cycles the call took (0 when unknown).
//!
//! This module also holds the firmware shells: the protocol loop (`<prefix>_hil.c`) and
//! board support for the host (stdio), the ATmega328P (Arduino Uno, bare avr-libc), the
//! ESP32 (bare metal, booted by the mask ROM, as in Espressif's QEMU) and an Arduino
//! sketch that builds for both boards with the Arduino IDE or `arduino-cli`.

use crate::codegen::Model;
use crate::embedded::{Manifest, RealType};
use crate::expr::VarId;
use crate::flat::VarKind;
use crate::sim::{SimOptions, Simulation};
use anyhow::{Result, anyhow, bail};
use std::fmt::Write;

/// One answer of the controller.
#[derive(Clone, Debug)]
pub struct Reply {
    pub outputs: Vec<f64>,
    pub status: u8,
    /// Processor cycles of the call, if the target counts them.
    pub cycles: u32,
}

/// The controller end of the loop.
pub trait Link {
    /// The first sample: initialise with these inputs.
    fn start(&mut self, inputs: &[f64]) -> Result<Reply>;
    /// Every later sample.
    fn step(&mut self, inputs: &[f64]) -> Result<Reply>;
}

#[derive(Clone, Debug, Default)]
pub struct LoopOptions {
    pub stop_time: Option<f64>,
    /// `controller_port=bench_variable` overrides of the by-name wiring.
    pub map: Vec<(String, String)>,
    /// Extra bench variables to record.
    pub record: Vec<String>,
    pub sim: Option<SimOptions>,
}

#[derive(Clone, Debug, Default)]
pub struct LoopResult {
    /// Column names: `time`, the controller's inputs and outputs, `cycles`, recorded bench variables.
    pub names: Vec<String>,
    pub rows: Vec<Vec<f64>>,
    pub max_cycles: u32,
    pub mean_cycles: f64,
    pub samples: usize,
}

impl LoopResult {
    pub fn to_csv(&self) -> String {
        let mut s = self.names.join(",");
        s.push('\n');
        for r in &self.rows {
            let cells: Vec<String> = r.iter().map(|v| format!("{v:.12e}")).collect();
            s.push_str(&cells.join(","));
            s.push('\n');
        }
        s
    }
}

fn find_var(m: &Model, name: &str) -> Option<VarId> {
    m.vars
        .iter()
        .position(|v| v.name == name)
        .map(|i| i as VarId)
}

/// Run the bench model `bench` in closed loop with the controller behind `link`.
/// Controller inputs are read from bench variables of the same name (bench outputs), and
/// controller outputs drive bench inputs of the same name, unless `opts.map` says otherwise.
pub fn closed_loop(
    bench: &Model,
    man: &Manifest,
    link: &mut dyn Link,
    opts: &LoopOptions,
) -> Result<LoopResult> {
    let lookup = |port: &str| -> String {
        opts.map
            .iter()
            .find(|(c, _)| c == port)
            .map(|(_, b)| b.clone())
            .unwrap_or_else(|| port.to_string())
    };
    let mut ins: Vec<VarId> = Vec::new();
    for p in &man.inputs {
        let b = lookup(&p.name);
        ins.push(find_var(bench, &b).ok_or_else(|| {
            anyhow!(
                "controller input '{}': the bench model has no variable '{b}' (use --map {}=VAR)",
                p.name,
                p.name
            )
        })?);
    }
    let mut outs: Vec<VarId> = Vec::new();
    for p in &man.outputs {
        let b = lookup(&p.name);
        let v = find_var(bench, &b).ok_or_else(|| {
            anyhow!(
                "controller output '{}': the bench model has no input '{b}' (use --map {}=INPUT)",
                p.name,
                p.name
            )
        })?;
        if bench.vars[v as usize].kind != VarKind::Input {
            bail!(
                "controller output '{}' drives '{b}', which is not a top-level input of the bench model",
                p.name
            );
        }
        outs.push(v);
    }
    let mut record: Vec<VarId> = Vec::new();
    for r in &opts.record {
        record.push(find_var(bench, r).ok_or_else(|| anyhow!("unknown bench variable '{r}'"))?);
    }
    let mut so = opts
        .sim
        .clone()
        .unwrap_or_else(|| SimOptions::from_model(bench));
    if let Some(t) = opts.stop_time {
        so.stop_time = t;
    }
    so.start_time = man.start_time;
    let h = man.step;
    let n_steps = ((so.stop_time - so.start_time) / h).round() as usize;
    let mut sim = Simulation::new(bench, so.clone());
    sim.initialize()
        .map_err(|e| anyhow!("bench initialization: {e}"))?;
    sim.rt.run(&bench.outputs);
    let mut names = vec!["time".to_string()];
    names.extend(man.inputs.iter().map(|p| p.name.clone()));
    names.extend(man.outputs.iter().map(|p| p.name.clone()));
    names.push("cycles".into());
    names.extend(opts.record.iter().cloned());
    let mut res = LoopResult {
        names,
        ..Default::default()
    };
    let mut st = sim.stepper();
    let mut sum_cycles = 0f64;
    for k in 0..=n_steps {
        if k > 0 {
            let t = so.start_time + k as f64 * h;
            sim.advance_to(&mut st, t)
                .map_err(|e| anyhow!("bench simulation at t = {t}: {e}"))?;
            if let Some(msg) = sim.terminated() {
                eprintln!("bench terminated: {msg}");
                break;
            }
        }
        let u: Vec<f64> = ins.iter().map(|&v| sim.rt.var(v)).collect();
        let rep = if k == 0 {
            link.start(&u)?
        } else {
            link.step(&u)?
        };
        if rep.outputs.len() != outs.len() {
            bail!(
                "controller sent {} outputs, expected {}",
                rep.outputs.len(),
                outs.len()
            );
        }
        if rep.status & 3 != 0 {
            bail!(
                "controller failed at t = {} (status {})",
                sim.rt.time(),
                rep.status
            );
        }
        for (&v, &y) in outs.iter().zip(&rep.outputs) {
            sim.rt.r[v as usize] = y;
        }
        // Inputs jumped: algebraics, then restart the integrator.
        let t_now = st.t;
        sim.rt.set_time(t_now);
        sim.rt.run(&bench.ode);
        sim.handle_event()
            .map_err(|e| anyhow!("bench event at t = {t_now}: {e}"))?;
        sim.rt.run(&bench.outputs);
        sim.restart(&mut st);
        let mut row = vec![t_now];
        row.extend(&u);
        row.extend(&rep.outputs);
        row.push(rep.cycles as f64);
        row.extend(record.iter().map(|&v| sim.rt.var(v)));
        res.rows.push(row);
        res.max_cycles = res.max_cycles.max(rep.cycles);
        if k > 0 {
            sum_cycles += rep.cycles as f64;
        }
        res.samples += 1;
        if rep.status & 4 != 0 {
            break;
        }
    }
    res.mean_cycles = if res.samples > 1 {
        sum_cycles / (res.samples - 1) as f64
    } else {
        0.0
    };
    Ok(res)
}

/// The protocol client over any byte stream.
pub struct StreamLink<R: std::io::Read, W: std::io::Write> {
    pub reader: R,
    pub writer: W,
    pub real_bytes: usize,
    pub n_in: usize,
    pub n_out: usize,
    /// Text received before the banner (boot messages).
    pub boot_log: String,
}

impl<R: std::io::Read, W: std::io::Write> StreamLink<R, W> {
    /// Wait for the banner, checking that the firmware matches the manifest.
    pub fn connect(mut reader: R, writer: W, man: &Manifest) -> Result<Self> {
        let mut log: Vec<u8> = Vec::new();
        let marker = b"DAEDALUS-HIL ";
        loop {
            let mut b = [0u8; 1];
            if reader.read(&mut b)? == 0 {
                bail!(
                    "the controller closed the connection before announcing itself; it printed:\n{}",
                    String::from_utf8_lossy(&log)
                );
            }
            log.push(b[0]);
            if b[0] == b'\n'
                && let Some(pos) = log.windows(marker.len()).rposition(|w| w == marker)
            {
                let line = String::from_utf8_lossy(&log[pos + marker.len()..])
                    .trim()
                    .to_string();
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() != 4 {
                    bail!("malformed banner '{line}'");
                }
                let (n_in, n_out, rb): (usize, usize, usize) =
                    (f[1].parse()?, f[2].parse()?, f[3].parse()?);
                if f[0] != man.prefix || n_in != man.inputs.len() || n_out != man.outputs.len() {
                    bail!(
                        "the firmware is '{}' with {n_in} inputs and {n_out} outputs, the manifest describes '{}' with {} and {}",
                        f[0],
                        man.prefix,
                        man.inputs.len(),
                        man.outputs.len()
                    );
                }
                if rb != 4 && rb != 8 {
                    bail!("unsupported real size {rb}");
                }
                let boot = String::from_utf8_lossy(&log[..pos]).to_string();
                return Ok(StreamLink {
                    reader,
                    writer,
                    real_bytes: rb,
                    n_in,
                    n_out,
                    boot_log: boot,
                });
            }
            if log.len() > 1 << 20 {
                bail!("no DAEDALUS-HIL banner in the first megabyte of output");
            }
        }
    }

    fn call(&mut self, cmd: u8, inputs: &[f64]) -> Result<Reply> {
        let mut frame = vec![0xA5u8, cmd];
        for &v in inputs.iter().take(self.n_in) {
            if self.real_bytes == 4 {
                frame.extend((v as f32).to_le_bytes());
            } else {
                frame.extend(v.to_le_bytes());
            }
        }
        self.writer.write_all(&frame)?;
        self.writer.flush()?;
        // Skip anything until the reply marker.
        let mut b = [0u8; 1];
        let mut skipped = 0usize;
        loop {
            self.reader.read_exact(&mut b)?;
            if b[0] == 0x5A {
                break;
            }
            skipped += 1;
            if skipped > 4096 {
                bail!("no reply from the controller");
            }
        }
        self.reader.read_exact(&mut b)?;
        let status = b[0];
        let mut outputs = Vec::with_capacity(self.n_out);
        for _ in 0..self.n_out {
            if self.real_bytes == 4 {
                let mut x = [0u8; 4];
                self.reader.read_exact(&mut x)?;
                outputs.push(f32::from_le_bytes(x) as f64);
            } else {
                let mut x = [0u8; 8];
                self.reader.read_exact(&mut x)?;
                outputs.push(f64::from_le_bytes(x));
            }
        }
        let mut c = [0u8; 4];
        self.reader.read_exact(&mut c)?;
        Ok(Reply {
            outputs,
            status,
            cycles: u32::from_le_bytes(c),
        })
    }
}

impl<R: std::io::Read, W: std::io::Write> Link for StreamLink<R, W> {
    fn start(&mut self, inputs: &[f64]) -> Result<Reply> {
        self.call(b'I', inputs)
    }
    fn step(&mut self, inputs: &[f64]) -> Result<Reply> {
        self.call(b'S', inputs)
    }
}

/// Firmware and build files, as (relative path, contents).
pub fn hil_files(man: &Manifest, header: &str, source: &str) -> Vec<(String, String)> {
    let p = &man.prefix;
    let mut v = Vec::new();
    v.push((format!("hil/{p}_hil.c"), protocol_c(man)));
    v.push(("hil/hil_hal.h".into(), HAL_H.into()));
    v.push(("hil/host/hal_stdio.c".into(), HAL_STDIO.into()));
    v.push(("hil/host/Makefile".into(), host_makefile(p)));
    v.push(("hil/avr/hal_avr.c".into(), HAL_AVR.into()));
    v.push(("hil/avr/Makefile".into(), avr_makefile(p)));
    v.push(("hil/esp32/hal_esp32.c".into(), HAL_ESP32.into()));
    v.push(("hil/esp32/esp32.ld".into(), ESP32_LD.into()));
    v.push(("hil/esp32/Makefile".into(), esp32_makefile(p)));
    // Arduino: a sketch folder must hold all its sources.
    let sk = format!("hil/arduino/{p}_hil");
    v.push((format!("{sk}/{p}_hil.ino"), arduino_ino(man)));
    v.push((format!("{sk}/{p}.h"), header.to_string()));
    v.push((format!("{sk}/{p}.c"), source.to_string()));
    v.push((format!("{sk}/hil_hal.h"), HAL_H.into()));
    v.push((format!("{sk}/{p}_hil.c"), protocol_c(man)));
    v.push(("hil/README.md".into(), readme(man)));
    v
}

fn protocol_c(man: &Manifest) -> String {
    let p = &man.prefix;
    let up = p.to_uppercase();
    format!(
        r#"/* Processor-in-the-loop protocol for {p} (see daedalus_core::embedded_hil).
 * Generated by DAEdalus. The board support (hil_hal.h) provides the serial port and a
 * cycle counter. */
#include <string.h>
#include "{p}.h"
#include "hil_hal.h"

static {p}_t hil_model;

static void put_real(real_t v)
{{
    uint8_t b[sizeof(real_t)];
    uint8_t i;
    memcpy(b, &v, sizeof b);
    for (i = 0; i < sizeof b; i++) hil_putc(b[i]);
}}

static real_t get_real(void)
{{
    uint8_t b[sizeof(real_t)];
    uint8_t i;
    real_t v;
    for (i = 0; i < sizeof b; i++) b[i] = (uint8_t)hil_getc();
    memcpy(&v, b, sizeof v);
    return v;
}}

static void put_str(const char *s)
{{
    while (*s) hil_putc((uint8_t)*s++);
}}

static void put_uint(unsigned int v)
{{
    char buf[8];
    int n = 0;
    do {{ buf[n++] = (char)('0' + v % 10u); v /= 10u; }} while (v && n < 7);
    while (n) hil_putc((uint8_t)buf[--n]);
}}

void {p}_hil_banner(void)
{{
    put_str("\nDAEDALUS-HIL {p} ");
    put_uint({up}_N_INPUTS);
    hil_putc(' ');
    put_uint({up}_N_OUTPUTS);
    hil_putc(' ');
    put_uint((unsigned int)sizeof(real_t));
    hil_putc('\n');
    hil_flush();
}}

/* Handle one frame; returns after the reply. */
void {p}_hil_poll(void)
{{
    uint8_t cmd, st;
    uint16_t i;
    uint32_t c0, c1;
    real_t in[{up}_N_INPUTS + 1];
    if (hil_getc() != 0xA5) return;
    cmd = (uint8_t)hil_getc();
    if (cmd != 'I' && cmd != 'S') return;
    for (i = 0; i < {up}_N_INPUTS; i++) in[i] = get_real();
    c0 = hil_cycles();
    if (cmd == 'I') {{
        {p}_init(&hil_model);
        for (i = 0; i < {up}_N_INPUTS; i++) {p}_set_input(&hil_model, i, in[i]);
        st = {p}_start(&hil_model);
    }} else {{
        for (i = 0; i < {up}_N_INPUTS; i++) {p}_set_input(&hil_model, i, in[i]);
        st = {p}_step(&hil_model);
    }}
    c1 = hil_cycles();
    hil_putc(0x5A);
    hil_putc(st);
    for (i = 0; i < {up}_N_OUTPUTS; i++) put_real({p}_get_output(&hil_model, i));
    c1 -= c0;
    for (i = 0; i < 4; i++) hil_putc((uint8_t)(c1 >> (8 * i)));
    hil_flush();
    (void)in;
}}

#if !defined(HIL_NO_MAIN) && !defined(ARDUINO)
int main(void)
{{
    hil_setup();
    {p}_hil_banner();
    for (;;) {p}_hil_poll();
}}
#endif
"#
    )
}

const HAL_H: &str = r#"/* Board support for the DAEdalus processor-in-the-loop shell. */
#ifndef HIL_HAL_H
#define HIL_HAL_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
void hil_setup(void);
int hil_getc(void);            /* blocking; one byte */
void hil_putc(uint8_t c);
void hil_flush(void);
uint32_t hil_cycles(void);     /* free-running processor cycle counter (0 if unknown) */
#ifdef __cplusplus
}
#endif
#endif
"#;

const HAL_STDIO: &str = r#"/* Host board support: the serial port is stdin/stdout (software in the loop). */
#include <stdio.h>
#include <stdlib.h>
#include "../hil_hal.h"

void hil_setup(void) {}
int hil_getc(void)
{
    int c = getchar();
    if (c == EOF) exit(0);
    return c;
}
void hil_putc(uint8_t c) { putchar(c); }
void hil_flush(void) { fflush(stdout); }
uint32_t hil_cycles(void) { return 0; }
"#;

const HAL_AVR: &str = r#"/* ATmega328P (Arduino Uno, 16 MHz) board support: USART0 at 115200 baud, Timer1
 * counting cycles / 8. Builds with avr-gcc and avr-libc, no Arduino core needed. */
#include <avr/io.h>
#include "../hil_hal.h"

#ifndef F_CPU
#define F_CPU 16000000UL
#endif
#define BAUD 115200UL

static uint16_t overflows;

void hil_setup(void)
{
    uint16_t ubrr = (uint16_t)((F_CPU / (8UL * BAUD)) - 1UL); /* double speed */
    UCSR0A = (uint8_t)(1 << U2X0);
    UBRR0H = (uint8_t)(ubrr >> 8);
    UBRR0L = (uint8_t)ubrr;
    UCSR0B = (uint8_t)((1 << RXEN0) | (1 << TXEN0));
    UCSR0C = (uint8_t)((1 << UCSZ01) | (1 << UCSZ00));
    TCCR1A = 0;
    TCCR1B = (uint8_t)(1 << CS11); /* clk / 8 */
}

int hil_getc(void)
{
    while (!(UCSR0A & (1 << RXC0))) {}
    return UDR0;
}

void hil_putc(uint8_t c)
{
    while (!(UCSR0A & (1 << UDRE0))) {}
    UDR0 = c;
}

void hil_flush(void) {}

uint32_t hil_cycles(void)
{
    uint16_t t = TCNT1;
    if (TIFR1 & (1 << TOV1)) {
        TIFR1 = (uint8_t)(1 << TOV1);
        overflows++;
        t = TCNT1;
    }
    return (((uint32_t)overflows << 16) | t) * 8UL;
}
"#;

const HAL_ESP32: &str = r#"/* ESP32 bare-metal board support: the mask ROM loads this program as a second-stage
 * bootloader (no ESP-IDF needed), UART0 at its boot settings, the CCOUNT cycle counter.
 * Runs on a real ESP32 and in Espressif's QEMU (qemu-system-xtensa -machine esp32). */
#include "../hil_hal.h"

#define REG(a) (*(volatile uint32_t *)(a))
#define UART0_FIFO 0x3FF40000u
#define UART0_STATUS 0x3FF4001Cu
#define WDT_KEY 0x50D83AA1u

static void wdt_off(void)
{
    /* Timer group 0/1 and RTC watchdogs: unlock, disable, lock. */
    REG(0x3FF5F064u) = WDT_KEY; REG(0x3FF5F048u) = 0; REG(0x3FF5F064u) = 0;
    REG(0x3FF60064u) = WDT_KEY; REG(0x3FF60048u) = 0; REG(0x3FF60064u) = 0;
    REG(0x3FF480A4u) = WDT_KEY; REG(0x3FF4808Cu) = 0; REG(0x3FF480A4u) = 0;
}

void hil_setup(void) { wdt_off(); }

int hil_getc(void)
{
    while ((REG(UART0_STATUS) & 0xFFu) == 0) {}
    return (int)(REG(UART0_FIFO) & 0xFFu);
}

void hil_putc(uint8_t c)
{
    while (((REG(UART0_STATUS) >> 16) & 0xFFu) >= 120u) {}
    REG(UART0_FIFO) = c;
}

void hil_flush(void) {}

uint32_t hil_cycles(void)
{
    uint32_t c;
    __asm__ volatile("rsr %0, ccount" : "=r"(c));
    return c;
}

/* Entry point: enable the floating-point coprocessor, then the protocol loop. */
extern int main(void);
extern uint32_t _bss_start, _bss_end;
void hil_entry(void)
{
    uint32_t cp = 1, *b;
    __asm__ volatile("wsr %0, cpenable\n rsync" ::"r"(cp));
    for (b = &_bss_start; b < &_bss_end; b++) *b = 0;
    main();
    for (;;) {}
}
"#;

const ESP32_LD: &str = r#"/* ESP32: code in IRAM, data in SRAM2, loaded by the mask ROM from flash offset 0x1000. */
ENTRY(hil_entry)
MEMORY
{
  iram (rwx) : ORIGIN = 0x40080400, LENGTH = 0x10000
  dram (rw)  : ORIGIN = 0x3FFB0000, LENGTH = 0x20000
}
SECTIONS
{
  .text : { *(.entry.text) *(.literal .text .literal.* .text.*) } > iram
  .rodata : { *(.rodata .rodata.* .srodata*) } > dram
  .data : { *(.data .data.* .sdata*) } > dram
  .bss (NOLOAD) : { _bss_start = .; *(.bss .bss.* .sbss* COMMON) _bss_end = .; } > dram
}
"#;

fn host_makefile(p: &str) -> String {
    format!(
        r#"# Software in the loop: the controller as a host program speaking the HIL protocol on
# stdin/stdout. Run with: daedalus hil BENCH... --controller ../../{p}.json --exec ./{p}_sil
CC ?= cc
CFLAGS ?= -O2 -std=c99 -Wall
{p}_sil: ../../{p}.c ../{p}_hil.c hal_stdio.c
	$(CC) $(CFLAGS) -I../.. -I.. -o $@ $^ -lm
clean:
	rm -f {p}_sil
"#
    )
}

fn avr_makefile(p: &str) -> String {
    format!(
        r#"# Arduino Uno (ATmega328P, 16 MHz) firmware with avr-gcc; flash with
#   avrdude -p m328p -c arduino -P /dev/ttyACM0 -b 115200 -U flash:w:{p}.hex
# or run it in simavr: daedalus hil ... --exec "simavr_bridge {p}.elf"
CC = avr-gcc
MCU ?= atmega328p
CFLAGS ?= -Os -std=c99 -mmcu=$(MCU) -DF_CPU=16000000UL -Wall -ffunction-sections -fdata-sections
{p}.hex: {p}.elf
	avr-objcopy -O ihex -R .eeprom $< $@
{p}.elf: ../../{p}.c ../{p}_hil.c hal_avr.c
	$(CC) $(CFLAGS) -I../.. -I.. -Wl,--gc-sections -o $@ $^ -lm
	avr-size $@
clean:
	rm -f {p}.elf {p}.hex
"#
    )
}

fn esp32_makefile(p: &str) -> String {
    format!(
        r#"# ESP32 bare-metal firmware (no ESP-IDF): the mask ROM boots it from flash offset 0x1000.
# Needs Espressif's xtensa-esp-elf toolchain and esptool (pip install esptool).
# Real board:  esptool --chip esp32 write-flash 0x1000 {p}.bin   (115200 baud on UART0)
# Emulator:    daedalus hil ... --exec "qemu-system-xtensa -machine esp32 -nographic -monitor none \
#                -serial stdio -drive file={p}_flash.bin,if=mtd,format=raw -nic none"
CC = xtensa-esp32-elf-gcc
CFLAGS ?= -O2 -std=c99 -mlongcalls -ffunction-sections -fdata-sections -Wall
ESPTOOL ?= esptool
all: {p}_flash.bin
{p}.elf: ../../{p}.c ../{p}_hil.c hal_esp32.c esp32.ld
	$(CC) $(CFLAGS) -I../.. -I.. -nostartfiles -T esp32.ld -Wl,--gc-sections -o $@ \
	  ../../{p}.c ../{p}_hil.c hal_esp32.c -lm -lc -lgcc
{p}.bin: {p}.elf
	$(ESPTOOL) --chip esp32 elf2image --flash-mode dio --flash-size 4MB -o $@ $<
{p}_flash.bin: {p}.bin
	$(ESPTOOL) --chip esp32 merge-bin --pad-to-size 4MB -o $@ 0x1000 $<
clean:
	rm -f {p}.elf {p}.bin {p}_flash.bin
"#
    )
}

fn arduino_ino(man: &Manifest) -> String {
    let p = &man.prefix;
    format!(
        r#"// DAEdalus processor-in-the-loop sketch for {p} (Arduino Uno, Nano, Mega, ESP32 boards).
// Open in the Arduino IDE or: arduino-cli compile --fqbn arduino:avr:uno {p}_hil
// Then run the plant on the PC: daedalus hil BENCH... --controller {p}.json --serial /dev/ttyACM0
#define HIL_NO_MAIN
extern "C" {{
#include "hil_hal.h"
void {p}_hil_banner(void);
void {p}_hil_poll(void);
}}

extern "C" void hil_setup(void) {{ Serial.begin(115200); while (!Serial) {{}} }}
extern "C" int hil_getc(void) {{ while (Serial.available() <= 0) {{}} return Serial.read(); }}
extern "C" void hil_putc(uint8_t c) {{ Serial.write(c); }}
extern "C" void hil_flush(void) {{ Serial.flush(); }}
extern "C" uint32_t hil_cycles(void) {{ return micros() * (F_CPU / 1000000UL); }}

void setup()
{{
    hil_setup();
    delay(100);
    {p}_hil_banner();
}}

void loop()
{{
    {p}_hil_poll();
}}
"#
    )
}

fn readme(man: &Manifest) -> String {
    let p = &man.prefix;
    let mut s = String::new();
    let _ = writeln!(s, "# Processor-in-the-loop shells for `{p}`\n");
    let _ = writeln!(
        s,
        "`{p}.c` advances the model `{}` by {} s per call. These folders wrap it in a\nserial protocol so that `daedalus hil` can close the loop with a plant model simulated by\nDAEdalus.\n",
        man.model, man.step
    );
    s.push_str("| Folder | Target | Build | Run the loop |\n| --- | --- | --- | --- |\n");
    let _ = writeln!(
        s,
        "| `host/` | this computer (software in the loop) | `make` | `--exec ./{p}_sil` |"
    );
    let _ = writeln!(
        s,
        "| `avr/` | Arduino Uno (ATmega328P), simavr | `make` (avr-gcc) | `--exec \"simavr_bridge {p}.elf\"`, or flash and `--serial /dev/ttyACM0` |"
    );
    let _ = writeln!(
        s,
        "| `esp32/` | ESP32, Espressif QEMU | `make` (xtensa-esp32-elf-gcc, esptool) | `--exec \"qemu-system-xtensa -machine esp32 -nographic -monitor none -serial stdio -drive file={p}_flash.bin,if=mtd,format=raw -nic none\"` |"
    );
    let _ = writeln!(
        s,
        "| `arduino/{p}_hil/` | any Arduino board (Uno, ESP32...) | Arduino IDE or `arduino-cli compile` | `--serial PORT` |"
    );
    let _ = writeln!(
        s,
        "\nThe instance struct takes about {} bytes of RAM ({} registers of `{}`).",
        man.instance_bytes,
        man.registers,
        match man.real {
            RealType::Float => "float",
            RealType::Double => "double",
        }
    );
    s
}
