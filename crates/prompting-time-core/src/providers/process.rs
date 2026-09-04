use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use super::ProviderError;

pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
pub const EVENT_CHANNEL_CAPACITY: usize = 256;
const STDERR_LIMIT_BYTES: usize = 64 * 1024;
const OUTBOUND_CHANNEL_CAPACITY: usize = 1;

enum OwnerCommand {
    Send {
        line: Vec<u8>,
        reply: oneshot::Sender<Result<(), ProviderError>>,
    },
}

/// An owned JSON-lines child process.
///
/// One owner task operates the child. Its dedicated stdin/stdout/stderr tasks are cancelled and
/// joined before the owner exits, so blocked pipe I/O cannot prevent child termination and reaping.
pub struct JsonLineProcess {
    id: u32,
    commands: mpsc::Sender<OwnerCommand>,
    shutdown: watch::Sender<bool>,
    events: mpsc::Receiver<Result<Value, ProviderError>>,
    stderr: Arc<Mutex<VecDeque<u8>>>,
    owner: Option<JoinHandle<Result<(), ProviderError>>>,
}

#[derive(Clone)]
pub struct JsonLineSender {
    commands: mpsc::Sender<OwnerCommand>,
}

#[derive(Clone)]
pub struct JsonLineShutdown {
    shutdown: watch::Sender<bool>,
}

impl JsonLineProcess {
    pub fn spawn(mut command: Command) -> Result<Self, ProviderError> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(transport_error)?;
        let id = child.id().ok_or_else(|| ProviderError::Transport {
            category: "missing-process-id".to_owned(),
        })?;
        let stdin = child.stdin.take().ok_or_else(|| ProviderError::Transport {
            category: "missing-stdin".to_owned(),
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProviderError::Transport {
                category: "missing-stdout".to_owned(),
            })?;
        let stderr_reader = child
            .stderr
            .take()
            .ok_or_else(|| ProviderError::Transport {
                category: "missing-stderr".to_owned(),
            })?;

        let (event_sender, events) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let (command_sender, commands) = mpsc::channel(OUTBOUND_CHANNEL_CAPACITY);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let stderr = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_LIMIT_BYTES)));
        let owner_stderr = Arc::clone(&stderr);
        let owner = tokio::spawn(async move {
            own_process(
                child,
                stdin,
                stdout,
                stderr_reader,
                OwnerChannels {
                    events: event_sender,
                    commands,
                    shutdown: shutdown_receiver,
                    stderr: owner_stderr,
                },
            )
            .await
        });

        Ok(Self {
            id,
            commands: command_sender,
            shutdown,
            events,
            stderr,
            owner: Some(owner),
        })
    }

    pub async fn send<T: Serialize>(&self, value: &T) -> Result<(), ProviderError> {
        self.sender().send(value).await
    }

    pub fn sender(&self) -> JsonLineSender {
        JsonLineSender {
            commands: self.commands.clone(),
        }
    }

    /// A process-owner shutdown signal that remains usable while another task is blocked on I/O.
    pub fn shutdown_handle(&self) -> JsonLineShutdown {
        JsonLineShutdown {
            shutdown: self.shutdown.clone(),
        }
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub async fn recv(&mut self) -> Option<Result<Value, ProviderError>> {
        self.events.recv().await
    }

    pub fn stderr_snapshot(&self) -> Vec<u8> {
        self.stderr
            .lock()
            .expect("stderr buffer mutex must not be poisoned")
            .iter()
            .copied()
            .collect()
    }

    pub async fn shutdown(mut self) -> Result<(), ProviderError> {
        self.shutdown.send_replace(true);
        match self.owner.take() {
            Some(owner) => owner.await.map_err(|_| owner_stopped())?,
            None => Ok(()),
        }
    }
}

impl JsonLineShutdown {
    pub fn request(&self) {
        self.shutdown.send_replace(true);
    }
}

impl JsonLineSender {
    pub async fn send<T: Serialize>(&self, value: &T) -> Result<(), ProviderError> {
        let permit = self.commands.reserve().await.map_err(|_| owner_stopped())?;
        let mut line = serde_json::to_vec(value).map_err(|_| ProviderError::Protocol {
            category: "request-serialization".to_owned(),
        })?;
        if line.len() > MAX_LINE_BYTES {
            return Err(ProviderError::OversizedFrame {
                limit: MAX_LINE_BYTES,
            });
        }
        line.push(b'\n');
        let (reply, response) = oneshot::channel();
        permit.send(OwnerCommand::Send { line, reply });
        response.await.map_err(|_| owner_stopped())?
    }
}

impl Drop for JsonLineProcess {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
    }
}

async fn own_process(
    mut child: tokio::process::Child,
    stdin: ChildStdin,
    stdout: impl AsyncRead + Unpin + Send + 'static,
    stderr_reader: impl AsyncRead + Unpin + Send + 'static,
    channels: OwnerChannels,
) -> Result<(), ProviderError> {
    let OwnerChannels {
        events,
        mut commands,
        mut shutdown,
        stderr,
    } = channels;
    let (cancel_sender, cancel_receiver) = watch::channel(false);
    let (fatal_sender, mut fatal_receiver) = mpsc::channel(1);
    let (write_sender, write_receiver) = mpsc::channel(OUTBOUND_CHANNEL_CAPACITY);
    let mut write_sender = Some(write_sender);
    let mut stdin_task = tokio::spawn(write_stdin(stdin, write_receiver, cancel_receiver.clone()));
    let mut stdout_task = tokio::spawn(read_stdout(
        stdout,
        events.clone(),
        cancel_receiver.clone(),
        fatal_sender,
    ));
    let mut stderr_task = tokio::spawn(read_stderr(stderr_reader, stderr, cancel_receiver));
    let mut child_status: Option<std::process::ExitStatus> = None;
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut stdin_done = false;

    loop {
        if let Some(status) = child_status
            && stdout_done
            && stderr_done
            && stdin_done
        {
            if !status.success() {
                return send_exit_error_or_shutdown(&events, &mut shutdown).await;
            }
            return Ok(());
        }
        tokio::select! {
            _ = shutdown.changed() => {
                let result = if child_status.is_none() {
                    kill_and_wait(&mut child).await
                } else {
                    Ok(())
                };
                cancel_and_join(
                    &cancel_sender,
                    &mut stdin_task,
                    &mut stdout_task,
                    &mut stderr_task,
                    stdin_done,
                    stdout_done,
                    stderr_done,
                )
                .await;
                return result;
            }
            status = child.wait(), if child_status.is_none() => {
                match status {
                    Ok(status) => {
                        child_status = Some(status);
                        write_sender.take();
                    }
                    Err(error) => {
                        cancel_and_join(
                            &cancel_sender,
                            &mut stdin_task,
                            &mut stdout_task,
                            &mut stderr_task,
                            stdin_done,
                            stdout_done,
                            stderr_done,
                        )
                        .await;
                        return Err(transport_error(error));
                    }
                }
            }
            result = &mut stdout_task, if !stdout_done => {
                stdout_done = true;
                if result.is_err() {
                    if child_status.is_none() {
                        kill_and_wait(&mut child).await?;
                    }
                    cancel_and_join(
                        &cancel_sender,
                        &mut stdin_task,
                        &mut stdout_task,
                        &mut stderr_task,
                        stdin_done,
                        stdout_done,
                        stderr_done,
                    )
                    .await;
                    return Err(reader_stopped("stdout-task"));
                }
            }
            result = &mut stderr_task, if !stderr_done => {
                stderr_done = true;
                if result.is_err() {
                    if child_status.is_none() {
                        kill_and_wait(&mut child).await?;
                    }
                    cancel_and_join(
                        &cancel_sender,
                        &mut stdin_task,
                        &mut stdout_task,
                        &mut stderr_task,
                        stdin_done,
                        stdout_done,
                        stderr_done,
                    )
                    .await;
                    return Err(reader_stopped("stderr-task"));
                }
            }
            result = &mut stdin_task, if !stdin_done => {
                stdin_done = true;
                match result {
                    Ok(Ok(())) if child_status.is_some() => {}
                    Ok(Ok(())) => {
                        kill_and_wait(&mut child).await?;
                        cancel_and_join(
                            &cancel_sender,
                            &mut stdin_task,
                            &mut stdout_task,
                            &mut stderr_task,
                            stdin_done,
                            stdout_done,
                            stderr_done,
                        ).await;
                        return Err(reader_stopped("stdin-task"));
                    }
                    Ok(Err(error)) => {
                        if child_status.is_none() {
                            kill_and_wait(&mut child).await?;
                        }
                        cancel_and_join(
                            &cancel_sender,
                            &mut stdin_task,
                            &mut stdout_task,
                            &mut stderr_task,
                            stdin_done,
                            stdout_done,
                            stderr_done,
                        ).await;
                        return Err(error);
                    }
                    Err(_) => {
                        if child_status.is_none() {
                            kill_and_wait(&mut child).await?;
                        }
                        cancel_and_join(
                            &cancel_sender,
                            &mut stdin_task,
                            &mut stdout_task,
                            &mut stderr_task,
                            stdin_done,
                            stdout_done,
                            stderr_done,
                        ).await;
                        return Err(reader_stopped("stdin-task"));
                    }
                }
            }
            Some(reason) = fatal_receiver.recv() => {
                if reason == StdoutFailure::UnexpectedEof {
                    let status = match child.try_wait() {
                        Ok(status) => status,
                        Err(error) => {
                            cancel_and_join(
                                &cancel_sender,
                                &mut stdin_task,
                                &mut stdout_task,
                                &mut stderr_task,
                                stdin_done,
                                stdout_done,
                                stderr_done,
                            )
                            .await;
                            return Err(transport_error(error));
                        }
                    };
                    if let Some(status) = status {
                        child_status = Some(status);
                        write_sender.take();
                        continue;
                    }
                }
                let reap_result = if child_status.is_none() {
                    kill_and_wait(&mut child).await
                } else {
                    Ok(())
                };
                cancel_and_join(
                    &cancel_sender,
                    &mut stdin_task,
                    &mut stdout_task,
                    &mut stderr_task,
                    stdin_done,
                    stdout_done,
                    stderr_done,
                )
                .await;
                reap_result?;
                return match reason {
                    StdoutFailure::Reported => Ok(()),
                    StdoutFailure::UnexpectedEof => {
                        send_error_or_shutdown(
                            &events,
                            ProviderError::StreamClosed,
                            &mut shutdown,
                        )
                        .await
                    }
                };
            }
            command = commands.recv() => {
                match command {
                    Some(OwnerCommand::Send { line, reply }) => {
                        if child_status.is_some() || write_sender.is_none() {
                            let _ = reply.send(Err(ProviderError::ProcessExited));
                            continue;
                        }
                        match write_sender
                            .as_ref()
                            .expect("writer exists while child is running")
                            .try_send(WriteRequest { line, reply })
                        {
                            Ok(()) => {}
                            Err(mpsc::error::TrySendError::Full(request)) => {
                                let error = ProviderError::Transport {
                                    category: "stdin-backpressure".to_owned(),
                                };
                                let _ = request.reply.send(Err(error));
                            }
                            Err(mpsc::error::TrySendError::Closed(request)) => {
                                let error = owner_stopped();
                                let _ = request.reply.send(Err(error));
                            }
                        }
                    }
                    None => {
                        if child_status.is_none() {
                            kill_and_wait(&mut child).await?;
                        }
                        cancel_and_join(
                            &cancel_sender,
                            &mut stdin_task,
                            &mut stdout_task,
                            &mut stderr_task,
                            stdin_done,
                            stdout_done,
                            stderr_done,
                        )
                        .await;
                        return Ok(());
                    }
                }
            }
        }
    }
}

struct OwnerChannels {
    events: mpsc::Sender<Result<Value, ProviderError>>,
    commands: mpsc::Receiver<OwnerCommand>,
    shutdown: watch::Receiver<bool>,
    stderr: Arc<Mutex<VecDeque<u8>>>,
}

async fn cancel_and_join(
    cancel: &watch::Sender<bool>,
    stdin: &mut JoinHandle<Result<(), ProviderError>>,
    stdout: &mut JoinHandle<()>,
    stderr: &mut JoinHandle<()>,
    stdin_done: bool,
    stdout_done: bool,
    stderr_done: bool,
) {
    cancel.send_replace(true);
    if !stdin_done {
        let _ = stdin.await;
    }
    if !stdout_done {
        join_reader(stdout).await;
    }
    if !stderr_done {
        join_reader(stderr).await;
    }
}

struct WriteRequest {
    line: Vec<u8>,
    reply: oneshot::Sender<Result<(), ProviderError>>,
}

async fn write_stdin(
    mut stdin: ChildStdin,
    mut writes: mpsc::Receiver<WriteRequest>,
    mut cancelled: watch::Receiver<bool>,
) -> Result<(), ProviderError> {
    loop {
        let request = tokio::select! {
            _ = cancelled.changed() => return Ok(()),
            request = writes.recv() => match request {
                Some(request) => request,
                None => return Ok(()),
            },
        };
        let result = tokio::select! {
            _ = cancelled.changed() => Err(owner_stopped()),
            result = stdin.write_all(&request.line) => result.map_err(transport_error),
        };
        let failed = result.is_err();
        let _ = request.reply.send(result.clone());
        if failed {
            return result;
        }
    }
}

async fn send_exit_error_or_shutdown(
    events: &mpsc::Sender<Result<Value, ProviderError>>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), ProviderError> {
    send_error_or_shutdown(events, ProviderError::ProcessExited, shutdown).await
}

async fn send_error_or_shutdown(
    events: &mpsc::Sender<Result<Value, ProviderError>>,
    error: ProviderError,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<(), ProviderError> {
    tokio::select! {
        result = events.send(Err(error)) => {
            result.map_err(|_| owner_stopped())?;
            Ok(())
        }
        _ = shutdown.changed() => Ok(()),
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum StdoutFailure {
    Reported,
    UnexpectedEof,
}

async fn read_stdout(
    mut stdout: impl AsyncRead + Unpin,
    events: mpsc::Sender<Result<Value, ProviderError>>,
    mut cancelled: watch::Receiver<bool>,
    fatal: mpsc::Sender<StdoutFailure>,
) {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let read = tokio::select! {
            changed = cancelled.changed() => {
                if changed.is_ok() && *cancelled.borrow() {
                    return;
                }
                continue;
            }
            result = stdout.read(&mut chunk) => result,
        };
        let count = match read {
            Ok(0) => {
                if !buffer.is_empty() && !emit_line(&buffer, &events, &fatal, &mut cancelled).await
                {
                    return;
                }
                let _ = fatal.send(StdoutFailure::UnexpectedEof).await;
                return;
            }
            Ok(count) => count,
            Err(_) => {
                let _ = send_event(
                    &events,
                    Err(ProviderError::Transport {
                        category: "stdout-read".to_owned(),
                    }),
                    &mut cancelled,
                )
                .await;
                let _ = fatal.send(StdoutFailure::Reported).await;
                return;
            }
        };

        for byte in &chunk[..count] {
            if *byte == b'\n' {
                if !buffer.is_empty() && !emit_line(&buffer, &events, &fatal, &mut cancelled).await
                {
                    return;
                }
                buffer.clear();
            } else {
                buffer.push(*byte);
                if buffer.strip_suffix(b"\r").unwrap_or(&buffer).len() > MAX_LINE_BYTES {
                    let _ = send_event(
                        &events,
                        Err(ProviderError::OversizedFrame {
                            limit: MAX_LINE_BYTES,
                        }),
                        &mut cancelled,
                    )
                    .await;
                    let _ = fatal.send(StdoutFailure::Reported).await;
                    return;
                }
            }
        }
    }
}

async fn emit_line(
    line: &[u8],
    events: &mpsc::Sender<Result<Value, ProviderError>>,
    fatal: &mpsc::Sender<StdoutFailure>,
    cancelled: &mut watch::Receiver<bool>,
) -> bool {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    match serde_json::from_slice(line) {
        Ok(value) => send_event(events, Ok(value), cancelled).await,
        Err(_) => {
            let _ = send_event(events, Err(ProviderError::MalformedJson), cancelled).await;
            let _ = fatal.send(StdoutFailure::Reported).await;
            false
        }
    }
}

async fn send_event(
    events: &mpsc::Sender<Result<Value, ProviderError>>,
    event: Result<Value, ProviderError>,
    cancelled: &mut watch::Receiver<bool>,
) -> bool {
    tokio::select! {
        _ = cancelled.changed() => false,
        result = events.send(event) => result.is_ok(),
    }
}

async fn read_stderr(
    mut reader: impl AsyncRead + Unpin,
    stderr: Arc<Mutex<VecDeque<u8>>>,
    mut cancelled: watch::Receiver<bool>,
) {
    let mut chunk = [0_u8; 4096];
    loop {
        let count = tokio::select! {
            changed = cancelled.changed() => {
                if changed.is_ok() && *cancelled.borrow() {
                    return;
                }
                continue;
            }
            result = reader.read(&mut chunk) => match result {
                Ok(count) => count,
                Err(_) => return,
            }
        };
        if count == 0 {
            return;
        }
        let mut buffer = stderr
            .lock()
            .expect("stderr buffer mutex must not be poisoned");
        for byte in &chunk[..count] {
            if buffer.len() == STDERR_LIMIT_BYTES {
                buffer.pop_front();
            }
            buffer.push_back(*byte);
        }
    }
}

async fn kill_and_wait(child: &mut tokio::process::Child) -> Result<(), ProviderError> {
    match child.try_wait().map_err(transport_error)? {
        Some(_) => Ok(()),
        None => {
            child.kill().await.map_err(transport_error)?;
            child.wait().await.map_err(transport_error)?;
            Ok(())
        }
    }
}

async fn join_reader(handle: &mut JoinHandle<()>) {
    let _ = handle.await;
}

fn transport_error(_: std::io::Error) -> ProviderError {
    ProviderError::Transport {
        category: "process-io".to_owned(),
    }
}

fn owner_stopped() -> ProviderError {
    ProviderError::Transport {
        category: "owner-stopped".to_owned(),
    }
}

fn reader_stopped(category: &'static str) -> ProviderError {
    ProviderError::Transport {
        category: category.to_owned(),
    }
}
