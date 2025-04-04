use std::time::Duration;

use anyhow::Context;
use backon::{ExponentialBuilder, Retryable};
use listenfd::ListenFd;
use poem::{
    EndpointExt, Route, Server, handler,
    http::StatusCode,
    listener::{DynAcceptor, Listener, TcpListener, ToDynAcceptor, UnixAcceptor},
    web::{Data, Query},
};
use serde::Deserialize;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::{io::AsyncWriteExt, time};
use tokio_serial::SerialStream;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

#[handler]
async fn post_toggle_power(tx: Data<&CommandSender>) -> poem::Result<()> {
    tx.send(UserCommand::Direct(InfraredCommand::TogglePower))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(())
}

#[handler]
async fn post_power_on_hack(tx: Data<&CommandSender>) -> poem::Result<()> {
    tx.send(UserCommand::PowerOnHack)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct SetInputParams {
    input: AudioInput,
}

#[handler]
async fn post_set_input(tx: Data<&CommandSender>, q: Query<SetInputParams>) -> poem::Result<()> {
    tx.send(UserCommand::Direct(InfraredCommand::SetInput(q.input)))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct RawCommandParams {
    cmd: u8,
}

#[handler]
async fn post_raw_command(
    tx: Data<&CommandSender>,
    q: Query<RawCommandParams>,
) -> poem::Result<()> {
    tx.send(UserCommand::Direct(InfraredCommand::Raw(q.cmd)))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(())
}

async fn make_acceptor() -> anyhow::Result<Box<dyn DynAcceptor>> {
    let mut listenfd = ListenFd::from_env();
    Ok(match listenfd.take_unix_listener(0)? {
        Some(listener) => {
            listener.set_nonblocking(true)?;
            Box::new(ToDynAcceptor(UnixAcceptor::from_std(listener)?))
        }
        None => {
            warn!("Did not receive Unix socket, falling back to TCP.");
            Box::new(ToDynAcceptor(
                TcpListener::bind("127.0.0.1:9912").into_acceptor().await?,
            ))
        }
    })
}

enum UserCommand {
    /// Directly transmit an infrared command
    Direct(InfraredCommand),

    /// The Power button is a toggle, so unless we know the current state,
    /// we cannot reliably turn the device On.
    /// However, the device ignores a repeated power-toggle command within
    /// a few seconds of turning on, whereas this window is shorter when turning
    /// off. So sending a second power-toggle command in this gap causes the
    /// device to eventually reach the On state, with the downside of a few
    /// seconds delay if it was already on.
    PowerOnHack,
}

enum InfraredCommand {
    TogglePower,
    SetInput(AudioInput),
    Raw(u8),
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum AudioInput {
    Bluetooth,
    #[serde(rename = "3.5mm")]
    _3_5mm,
    Optical,
    Rca,
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

#[derive(Clone)]
struct CommandSender(Sender<UserCommand>);

impl CommandSender {
    async fn send(&self, command: UserCommand) -> Result<(), ()> {
        const CMD_TIMEOUT: Duration = Duration::from_secs(5);

        self.0
            .send_timeout(command, CMD_TIMEOUT)
            .await
            .map_err(|_| ())
    }
}

async fn open_serial() -> anyhow::Result<SerialStream> {
    let s = (async || -> anyhow::Result<SerialStream> {
        tokio::task::spawn_blocking(|| {
            Ok(tokio_serial::SerialStream::open(&tokio_serial::new(
                "/dev/serial/by-id/usb-Jabu_Infrared_1-if00",
                115200,
            ))?)
        })
        .await?
    })
    .retry(ExponentialBuilder::default().with_max_times(16))
    .notify(|e, d| warn!("Failed to open serial, retrying in {} s: {e}", d.as_secs()))
    .await
    .context("Could not open serial port")?;
    Ok(s)
}

async fn ir_task(mut rx: Receiver<UserCommand>) -> anyhow::Result<()> {
    async fn ir(serial: &mut SerialStream, cmd: InfraredCommand) -> anyhow::Result<()> {
        let v = cmd.as_u32_le();
        let hex = format!("{v:x}");
        debug!("Sending command: {hex}");
        while let Err(e) = serial.write_all(hex.as_bytes()).await {
            error!("Failed to write to serial, reopening: {e:?}");
            *serial = open_serial().await?;
        }
        Ok(())
    }

    let mut serial = open_serial().await?;
    loop {
        let Some(cmd) = rx.recv().await else {
            // All senders died, we're done here
            return Ok(());
        };
        match cmd {
            UserCommand::Direct(v) => ir(&mut serial, v).await?,
            UserCommand::PowerOnHack => {
                ir(&mut serial, InfraredCommand::TogglePower).await?;
                time::sleep(Duration::from_secs_f32(3.)).await;
                ir(&mut serial, InfraredCommand::TogglePower).await?;
                time::sleep(Duration::from_secs_f32(3.)).await;
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let (tx, rx) = mpsc::channel::<UserCommand>(1);
    let app = Route::new()
        .at("/toggle-power", poem::post(post_toggle_power))
        .at("/power-on-hack", poem::post(post_power_on_hack))
        .at("/set-input", poem::post(post_set_input))
        .at("/raw-command", poem::post(post_raw_command))
        .data(CommandSender(tx));
    let acceptor = make_acceptor().await?;

    let cancel_token = CancellationToken::new();

    let cancel_token_ir = cancel_token.clone();
    tokio::spawn(async move {
        if let Err(e) = ir_task(rx).await {
            error!("IR Task died, cleaning up: {e:#}");
            cancel_token_ir.cancel();
        }
    });

    Server::new_with_acceptor(acceptor)
        .run_with_graceful_shutdown(app, cancel_token.cancelled(), None)
        .await?;
    Ok(())
}
