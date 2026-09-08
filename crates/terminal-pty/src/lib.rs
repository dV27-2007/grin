//! PTY lifecycle and blocking I/O isolated from the application thread.

use std::{
    env,
    io::{self, Read, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use portable_pty::{CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use thiserror::Error;

const EVENT_CAPACITY: usize = 256;
const COMMAND_CAPACITY: usize = 256;
const READ_BUFFER_SIZE: usize = 16 * 1024;
const MAX_EVENTS_PER_DRAIN: usize = 16;

#[derive(Debug)]
pub enum PtyEvent {
    Output(Vec<u8>),
    Exited { success: bool },
    Error(String),
}

#[derive(Debug)]
enum PtyCommand {
    Write(Vec<u8>),
    Resize(PtySize),
    Shutdown,
}

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("could not open a pseudo-terminal: {0}")]
    Open(#[source] anyhow::Error),
    #[error("could not clone the pseudo-terminal reader: {0}")]
    CloneReader(#[source] anyhow::Error),
    #[error("could not acquire the pseudo-terminal writer: {0}")]
    TakeWriter(#[source] anyhow::Error),
    #[error("could not launch shell {shell}: {source}")]
    Spawn {
        shell: String,
        #[source]
        source: anyhow::Error,
    },
    #[error("the PTY command worker has stopped")]
    Disconnected,
    #[error("the PTY command queue is full")]
    Busy,
}

pub struct PtySession {
    commands: Sender<PtyCommand>,
    events: Receiver<PtyEvent>,
    notification_pending: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl PtySession {
    pub fn spawn(
        columns: u16,
        rows: u16,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, PtyError> {
        Self::spawn_in(columns, rows, None, wake)
    }

    pub fn spawn_in(
        columns: u16,
        rows: u16,
        working_directory: Option<PathBuf>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, PtyError> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(PtyError::Open)?;

        let shell = default_shell();
        let mut command = CommandBuilder::new(&shell);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        if let Some(directory) = working_directory.filter(|directory| directory.is_dir()) {
            command.cwd(directory);
        } else if let Ok(directory) = env::current_dir() {
            command.cwd(directory);
        }

        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|source| PtyError::Spawn {
                shell: shell.display().to_string(),
                source,
            })?;
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(PtyError::CloneReader)?;
        let writer = pair.master.take_writer().map_err(PtyError::TakeWriter)?;
        let (event_tx, events) = bounded(EVENT_CAPACITY);
        let (commands, command_rx) = bounded(COMMAND_CAPACITY);
        let notification_pending = Arc::new(AtomicBool::new(false));
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let event_sender = EventSender {
            channel: event_tx,
            notification_pending: Arc::clone(&notification_pending),
            wake: Arc::clone(&wake),
        };

        spawn_reader(reader, event_sender.clone());
        spawn_controller(pair.master, writer, command_rx, event_sender.clone());
        thread::Builder::new()
            .name("pty-waiter".into())
            .spawn(move || match child.wait() {
                Ok(status) => {
                    let _ = event_sender.send(PtyEvent::Exited {
                        success: status.success(),
                    });
                }
                Err(error) => {
                    let _ = event_sender.send(PtyEvent::Error(format!(
                        "waiting for shell failed: {error}"
                    )));
                }
            })
            .expect("operating system refused to start PTY waiter thread");

        Ok(Self {
            commands,
            events,
            notification_pending,
            wake,
        })
    }

    /// Drains currently queued events. Clearing the notification flag before the
    /// drain makes a concurrently arriving event issue another wake-up.
    pub fn drain_events(&self) -> Vec<PtyEvent> {
        self.notification_pending.store(false, Ordering::Release);
        let events: Vec<_> = self.events.try_iter().take(MAX_EVENTS_PER_DRAIN).collect();
        if !self.events.is_empty() && !self.notification_pending.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
        events
    }

    pub fn write(&self, bytes: impl Into<Vec<u8>>) -> Result<(), PtyError> {
        self.try_command(PtyCommand::Write(bytes.into()))
    }

    pub fn resize(&self, columns: u16, rows: u16) -> Result<(), PtyError> {
        self.try_command(PtyCommand::Resize(PtySize {
            rows,
            cols: columns,
            pixel_width: 0,
            pixel_height: 0,
        }))
    }

    fn try_command(&self, command: PtyCommand) -> Result<(), PtyError> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                TrySendError::Full(_) => PtyError::Busy,
                TrySendError::Disconnected(_) => PtyError::Disconnected,
            })
    }
}

#[derive(Clone)]
struct EventSender {
    channel: Sender<PtyEvent>,
    notification_pending: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl EventSender {
    fn send(&self, event: PtyEvent) -> Result<(), ()> {
        self.channel.send(event).map_err(|_| ())?;
        if !self.notification_pending.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
        Ok(())
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.commands.try_send(PtyCommand::Shutdown);
    }
}

fn spawn_reader(mut reader: Box<dyn Read + Send>, events: EventSender) {
    thread::Builder::new()
        .name("pty-reader".into())
        .spawn(move || {
            let mut buffer = vec![0; READ_BUFFER_SIZE];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if events
                            .send(PtyEvent::Output(buffer[..read].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if is_pty_eof(&error) => break,
                    Err(error) => {
                        let _ = events.send(PtyEvent::Error(format!("PTY read failed: {error}")));
                        break;
                    }
                }
            }
        })
        .expect("operating system refused to start PTY reader thread");
}

fn is_pty_eof(error: &io::Error) -> bool {
    #[cfg(unix)]
    {
        // BSD and Linux PTYs commonly report EIO after the slave closes.
        error.raw_os_error() == Some(5)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

fn spawn_controller(
    master: Box<dyn MasterPty + Send>,
    mut writer: Box<dyn Write + Send>,
    commands: Receiver<PtyCommand>,
    events: EventSender,
) {
    thread::Builder::new()
        .name("pty-controller".into())
        .spawn(move || {
            while let Ok(command) = commands.recv() {
                let result = match command {
                    PtyCommand::Write(bytes) => {
                        writer.write_all(&bytes).and_then(|_| writer.flush())
                    }
                    PtyCommand::Resize(size) => master
                        .resize(size)
                        .map_err(|error| io::Error::other(error.to_string())),
                    PtyCommand::Shutdown => break,
                };
                if let Err(error) = result {
                    let _ = events.send(PtyEvent::Error(format!("PTY operation failed: {error}")));
                    break;
                }
            }
        })
        .expect("operating system refused to start PTY controller thread");
}

fn default_shell() -> PathBuf {
    #[cfg(windows)]
    {
        env::var_os("COMSPEC")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("powershell.exe"))
    }
    #[cfg(not(windows))]
    {
        env::var_os("SHELL")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/bin/sh"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_is_not_empty() {
        assert!(!default_shell().as_os_str().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn shell_input_round_trips_through_pty() {
        use std::{
            sync::mpsc,
            time::{Duration, Instant},
        };

        let (wake_tx, wake_rx) = mpsc::sync_channel(1);
        let session = PtySession::spawn(80, 24, move || {
            let _ = wake_tx.try_send(());
        })
        .expect("test shell should start");
        // The literal marker is deliberately assembled by the shell so a
        // terminal-driver echo of the command cannot satisfy the assertion.
        session
            .write(
                b"printf '\\107\\122\\111\\116\\137\\120\\124\\131\\137\\117\\113\\n'\r".to_vec(),
            )
            .expect("test command should be queued");

        let deadline = Instant::now() + Duration::from_secs(8);
        let mut output = Vec::new();
        while Instant::now() < deadline && !output.windows(11).any(|bytes| bytes == b"GRIN_PTY_OK")
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            wake_rx
                .recv_timeout(remaining)
                .expect("shell did not produce output before the timeout");
            for event in session.drain_events() {
                if let PtyEvent::Output(bytes) = event {
                    output.extend(bytes);
                }
            }
        }

        assert!(
            output.windows(11).any(|bytes| bytes == b"GRIN_PTY_OK"),
            "shell output did not contain the round-trip marker"
        );
        let _ = session.write(b"exit\r".to_vec());
    }
}
