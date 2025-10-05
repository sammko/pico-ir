use ::std::str::{self, FromStr};

use ::anyhow::{Context, anyhow, bail};
use ::rumqttc as mq;

#[derive(Clone, Copy, Debug)]
enum InfraredCommand {
    TogglePower,
    SetInput(AudioInput),
    Raw(u8),
}

#[derive(Clone, Copy, Debug)]
enum AudioInput {
    Bluetooth,
    _3_5mm,
    Optical,
    Rca,
}

impl FromStr for AudioInput {
    type Err = ::anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bluetooth" => Ok(Self::Bluetooth),
            "3.5mm" => Ok(Self::_3_5mm),
            "optical" => Ok(Self::Optical),
            "rca" => Ok(Self::Rca),
            _ => Err(anyhow!("invalid audio input string")),
        }
    }
}

impl InfraredCommand {
    pub fn as_u8(&self) -> u8 {
        match self {
            InfraredCommand::TogglePower => 0x66,
            InfraredCommand::SetInput(AudioInput::Bluetooth) => 0x86,
            InfraredCommand::SetInput(AudioInput::_3_5mm) => 0x97,
            InfraredCommand::SetInput(AudioInput::Optical) => 0x88,
            InfraredCommand::SetInput(AudioInput::Rca) => 0x96,
            InfraredCommand::Raw(b) => *b,
        }
    }

    pub fn as_u32_le(&self) -> u32 {
        const ADDRESS: u32 = 0x2385;
        (self.as_u8() as u32) << 24 | (!self.as_u8() as u32) << 16 | ADDRESS
    }
}

impl TryFrom<mq::Publish> for InfraredCommand {
    type Error = ::anyhow::Error;

    fn try_from(msg: mq::Publish) -> Result<Self, Self::Error> {
        let Some(topic) = msg.topic.strip_prefix("jabu/pico-ir/") else {
            bail!("topic prefix wrong");
        };
        let command = match topic {
            "power" => InfraredCommand::TogglePower,
            "input" => InfraredCommand::SetInput(str::from_utf8(&msg.payload)?.parse()?),
            "raw" => InfraredCommand::Raw(u8::from_str_radix(str::from_utf8(&msg.payload)?, 16)?),
            cmd => bail!("invalid command '{cmd}'"),
        };
        Ok(command)
    }
}

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
        let rumqttc::Event::Incoming(mq::Packet::Publish(msg)) = ev else {
            continue;
        };
        let command = match InfraredCommand::try_from(msg) {
            Ok(command) => command,
            Err(e) => {
                eprintln!("failed to parse message: {e}");
                continue;
            }
        };
        write!(serial, "{:x}", command.as_u32_le()).context("failed to write to serial port")?;
    }
    bail!("wtf loop died");
}
