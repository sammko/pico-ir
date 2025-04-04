Pico IR
===

A small project to control the on/off state and Audio Input of my speaker set
over an API. The goal is so that they can be automatically turned on when
a `spotifyd` session is started and a "smart speaker" experience attained.

The hardware used is a RP2350 Pico board (Pimoroni Plus variant, but that's
not really relevant) and an Adafruit IR transceiver, but any old IR led should
just about work for the transmitter part. The modulation is done by the RP2350,
not the transceiver board.

The speakers use a standard NEC 38 kHz infrared protocol which was first decoded
using CircuitPy library and just manually noting things down. Some bit ordering
confusion later and adapting the NEC transmitter PIO programs from
[raspberrypi/pico-examples](https://github.com/raspberrypi/pico-examples/tree/master/pio/ir_nec/nec_transmit_library)
and we have the first part down.

Secondly a tiny API server is created that takes control of the Linux-side serial
port and provides a more friendly HTTP API for consumers.