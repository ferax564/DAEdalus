/* simavr_bridge: run AVR firmware in simavr with USART0 connected to stdin/stdout.
 *
 *   simavr_bridge firmware.elf [mcu] [frequency]     (default atmega328p, 16 MHz)
 *
 * Used by `daedalus hil --exec "simavr_bridge pid.elf"` to run generated controller code
 * on an emulated Arduino Uno, cycle-accurate, in closed loop with a plant simulated by
 * DAEdalus. Build: cc -O2 -o simavr_bridge simavr_bridge.c -lsimavr -lelf
 */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <simavr/avr_uart.h>
#include <simavr/sim_avr.h>
#include <simavr/sim_elf.h>
#include <simavr/sim_irq.h>

static int xon = 1;
static unsigned char out[4096];
static size_t n_out;

static void flush_out(void)
{
    size_t k = 0;
    while (k < n_out) {
        ssize_t w = write(1, out + k, n_out - k);
        if (w < 0) {
            if (errno == EINTR) continue;
            exit(0);
        }
        k += (size_t)w;
    }
    n_out = 0;
}

static void on_output(struct avr_irq_t *irq, uint32_t value, void *param)
{
    (void)irq;
    (void)param;
    if (n_out == sizeof out) flush_out();
    out[n_out++] = (unsigned char)value;
}

static void on_xon(struct avr_irq_t *irq, uint32_t value, void *param)
{
    (void)irq; (void)value; (void)param;
    xon = 1;
}

static void on_xoff(struct avr_irq_t *irq, uint32_t value, void *param)
{
    (void)irq; (void)value; (void)param;
    xon = 0;
}

int main(int argc, char **argv)
{
    elf_firmware_t fw;
    avr_t *avr;
    const char *mcu = argc > 2 ? argv[2] : "atmega328p";
    unsigned long freq = argc > 3 ? strtoul(argv[3], NULL, 10) : 16000000UL;
    unsigned char in[4096];
    size_t n_in = 0, pos = 0;
    unsigned long it = 0;
    uint32_t flags = 0;
    avr_irq_t *rx;
    if (argc < 2) {
        fprintf(stderr, "usage: %s firmware.elf [mcu] [frequency]\n", argv[0]);
        return 2;
    }
    memset(&fw, 0, sizeof fw);
    if (elf_read_firmware(argv[1], &fw) != 0) {
        fprintf(stderr, "simavr_bridge: cannot read %s\n", argv[1]);
        return 2;
    }
    avr = avr_make_mcu_by_name(fw.mmcu[0] ? fw.mmcu : mcu);
    if (!avr) {
        fprintf(stderr, "simavr_bridge: unknown MCU %s\n", mcu);
        return 2;
    }
    avr_init(avr);
    avr->frequency = fw.frequency ? fw.frequency : freq;
    avr->log = 0;
    avr_load_firmware(avr, &fw);
    avr_ioctl(avr, AVR_IOCTL_UART_GET_FLAGS('0'), &flags);
    flags &= ~(uint32_t)(AVR_UART_FLAG_STDIO | AVR_UART_FLAG_POLL_SLEEP);
    avr_ioctl(avr, AVR_IOCTL_UART_SET_FLAGS('0'), &flags);
    avr_irq_register_notify(avr_io_getirq(avr, AVR_IOCTL_UART_GETIRQ('0'), UART_IRQ_OUTPUT), on_output, NULL);
    avr_irq_register_notify(avr_io_getirq(avr, AVR_IOCTL_UART_GETIRQ('0'), UART_IRQ_OUT_XON), on_xon, NULL);
    avr_irq_register_notify(avr_io_getirq(avr, AVR_IOCTL_UART_GETIRQ('0'), UART_IRQ_OUT_XOFF), on_xoff, NULL);
    rx = avr_io_getirq(avr, AVR_IOCTL_UART_GETIRQ('0'), UART_IRQ_INPUT);
    for (;;) {
        int state = avr_run(avr);
        if (state == cpu_Done || state == cpu_Crashed) {
            flush_out();
            fprintf(stderr, "simavr_bridge: the MCU stopped (%s)\n", state == cpu_Done ? "done" : "crashed");
            return state == cpu_Done ? 0 : 1;
        }
        if (pos < n_in && xon) {
            avr_raise_irq(rx, in[pos++]);
            continue;
        }
        if (++it % 2048 == 0 || (pos >= n_in && n_out)) {
            struct pollfd p = {0, POLLIN, 0};
            int wait = 0;
            if (n_out) flush_out();
            if (pos >= n_in && poll(&p, 1, wait) > 0) {
                ssize_t r = read(0, in, sizeof in);
                if (r <= 0) return 0;
                n_in = (size_t)r;
                pos = 0;
            }
        }
    }
}
