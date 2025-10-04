use ::rumqttc as mq;
use anyhow::{Context, bail};

fn main() -> ::anyhow::Result<()> {
    let mut serial = ::serialport::new("/dev/serial/by-id/usb-Jabu_Infrared_1-if00", 115200)
        .open()
        .context("serialport failed")?;
    let opts = {
        let mut opts = mq::MqttOptions::new("pico-ir-mqtt", "jabu.elver-vibe.ts.net", 1883);
        opts.set_credentials("pico-ir", "jozefjozef");
        opts
    };
    let (client, mut conn) = mq::Client::new(opts, 10);
    client.subscribe("jabu/pico-ir/#", mq::QoS::AtMostOnce)?;
    for ev in conn.iter() {
        let ev = ev.context("got connection error")?;
        match ev {
            rumqttc::Event::Incoming(mq::Packet::Publish(mq::Publish { topic, .. }))
                if topic == "jabu/pico-ir/power" =>
            {
                serial.write(b"66992385").context("serial write failed")?;
                println!("sent");
            }
            _ => {}
        }
    }
    bail!("wtf loop died");
}
