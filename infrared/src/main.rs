#![no_std]
#![no_main]

use core::str;

use defmt::{error, info, unwrap};
use embassy_executor::Spawner;
use embassy_rp::{
    bind_interrupts,
    clocks::clk_sys_freq,
    peripherals::{PIO0, USB},
    pio::{self, FifoJoin, Pio, program::pio_asm},
    usb,
};
use embassy_usb::{UsbDevice, class::cdc_acm};
use fixed::traits::ToFixed as _;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

// Program metadata for `picotool info`.
// This isn't needed, but it's recomended to have these minimal entries.
#[unsafe(link_section = ".bi_entries")]
#[used]
pub static PICOTOOL_ENTRIES: [embassy_rp::binary_info::EntryAddr; 4] = [
    embassy_rp::binary_info::rp_program_name!(c"Pico IR"),
    embassy_rp::binary_info::rp_program_description!(c"Transmits NEC IR protocol commands"),
    embassy_rp::binary_info::rp_cargo_version!(),
    embassy_rp::binary_info::rp_program_build_attribute!(),
];

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    USBCTRL_IRQ => usb::InterruptHandler<USB>;
});

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    let pio = p.PIO0;
    let mut pio = Pio::new(pio, Irqs);

    let usb_driver = usb::Driver::new(p.USB, Irqs);
    let usb_config = {
        let mut config = embassy_usb::Config::new(0xc0de, 0xcafe);
        config.manufacturer = Some("Jabu");
        config.product = Some("Infrared");
        config.serial_number = Some("1");
        config.max_power = 100;
        config.max_packet_size_0 = 64;
        config
    };

    // Create embassy-usb DeviceBuilder using the driver and config.
    // It needs some buffers for building the descriptors.
    let mut builder = {
        static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();

        
        embassy_usb::Builder::new(
            usb_driver,
            usb_config,
            CONFIG_DESCRIPTOR.init([0; 256]),
            BOS_DESCRIPTOR.init([0; 256]),
            &mut [], // no msos descriptors
            CONTROL_BUF.init([0; 64]),
        )
    };

    let mut class = {
        static STATE: StaticCell<cdc_acm::State> = StaticCell::new();
        let state = STATE.init(cdc_acm::State::new());
        cdc_acm::CdcAcmClass::new(&mut builder, state, 64)
    };

    let usb = builder.build();
    unwrap!(spawner.spawn(usb_task(usb)));

    // The PIO programs come from here https://github.com/raspberrypi/pico-examples/tree/master/pio/ir_nec/nec_transmit_library

    let prg_burst = pio_asm!(
        r#"
.define NUM_CYCLES 21               ; how many carrier cycles to generate
.define BURST_IRQ 7                 ; which IRQ should trigger a carrier burst
.define public TICKS_PER_LOOP 4     ; the number of instructions in the loop (for timing)

.wrap_target
    set X, (NUM_CYCLES - 1)         ; initialise the loop counter
    wait 1 irq BURST_IRQ            ; wait for the IRQ then clear it
cycle_loop:
    set pins, 1                     ; set the pin high (1 cycle)
    set pins, 0 [1]                 ; set the pin low (2 cycles)
    jmp X--, cycle_loop             ; (1 more cycle)
.wrap
    "#
    );

    let prg_control = pio_asm!(
        r#"
.define BURST_IRQ 7                     ; the IRQ used to trigger a carrier burst
.define NUM_INITIAL_BURSTS 16           ; how many bursts to transmit for a 'sync burst'

.wrap_target
    pull                                ; fetch a data word from the transmit FIFO into the
                                        ; output shift register, blocking if the FIFO is empty

    set X, (NUM_INITIAL_BURSTS - 1)     ; send a sync burst (9ms)
long_burst:
    irq BURST_IRQ
    jmp X-- long_burst

    nop [15]                            ; send a 4.5ms space
    irq BURST_IRQ [1]                   ; send a 562.5us burst to begin the first data bit

data_bit:
    out X, 1                            ; shift the least-significant bit from the OSR
    jmp !X burst                        ; send a short delay for a '0' bit
    nop [3]                             ; send an additional delay for a '1' bit
burst:
    irq BURST_IRQ                       ; send a 562.5us burst to end the data bit

jmp !OSRE data_bit                      ; continue sending bits until the OSR is empty

.wrap                                   ; fetch another data word from the FIFO
    "#
    );

    {
        let mut cfg = pio::Config::default();
        cfg.use_program(&pio.common.load_program(&prg_burst.program), &[]);
        let out_pin = pio.common.make_pio_pin(p.PIN_5);
        cfg.set_set_pins(&[&out_pin]);
        pio.sm0.set_pin_dirs(pio::Direction::Out, &[&out_pin]);
        cfg.clock_divider = ((clk_sys_freq() as f64)
            / (38222. * (prg_burst.public_defines.TICKS_PER_LOOP as f64)))
            .to_fixed();
        pio.sm0.set_config(&cfg);
        pio.sm0.set_enable(true);
    }

    let tick_rate = 2. * (1. / 562.5e-6);

    {
        let mut cfg = pio::Config::default();
        cfg.use_program(&pio.common.load_program(&prg_control.program), &[]);
        cfg.fifo_join = FifoJoin::TxOnly;
        cfg.clock_divider = ((clk_sys_freq() as f64) / tick_rate).to_fixed();
        pio.sm1.set_config(&cfg);
        pio.sm1.set_enable(true);
    }

    info!("Hi");
    let mut buf = [0; 64];
    loop {
        let sz = class.read_packet(&mut buf).await.unwrap();
        if sz == 0 {
            continue;
        }
        let data = str::from_utf8(&buf[..sz]).unwrap();
        let Ok(value) = u32::from_str_radix(data, 16) else {
            error!("Can't parse hex u32: {:?}", data);
            continue;
        };
        info!("sz: {}, value: {:x}", sz, value);
        pio.sm1.tx().push(value);
    }
}

#[embassy_executor::task]
async fn usb_task(mut usb: UsbDevice<'static, usb::Driver<'static, USB>>) -> ! {
    usb.run().await
}
