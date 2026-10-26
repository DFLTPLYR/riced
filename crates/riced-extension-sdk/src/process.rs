//! Opt-in supervised executable services using bounded JSON-lines stdio.
//!
//! Workers use [`serve`]; hosts register a [`ProcessExtension`]. Stdout is
//! reserved for protocol frames, and stderr is inherited for diagnostics.
use crate::{Cancellation, Descriptor, Error, Extension, Manifest, Output, SDK_VERSION, Service};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::{BufRead, Read, Write},
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
};

/// Wire protocol version, independent of the Rust source SDK version.
pub const PROTOCOL_VERSION: u32 = 2;
// JSON escaping/envelope overhead plus the separately bounded output snapshot.
const FRAME_BYTES: usize = crate::MAX_SNAPSHOT_BYTES + crate::MAX_COMMAND_BYTES + 4096;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Frame {
    Hello {
        version: u32,
    },
    Ready {
        version: u32,
        service: String,
        commands: Vec<String>,
    },
    Command {
        id: u64,
        method: String,
        args: Value,
    },
    Success {
        id: u64,
        output: Output,
    },
    Failure {
        id: u64,
        error: Error,
    },
    /// Host asks the worker for its current unsolicited snapshot.
    Refresh {
        id: u64,
    },
    /// Worker's reply to a [`Frame::Refresh`]; `None` means no change.
    Snapshot {
        id: u64,
        snapshot: Option<Value>,
    },
}

fn protocol(message: impl Into<String>) -> Error {
    Error::new("protocol", message)
}
fn worker(error: impl std::fmt::Display) -> Error {
    Error::new("worker", error.to_string())
}

fn encode(frame: &Frame) -> Result<Vec<u8>, Error> {
    let mut bytes = serde_json::to_vec(frame).map_err(protocol_error)?;
    if bytes.len() > FRAME_BYTES {
        return Err(protocol("frame exceeds byte limit"));
    }
    bytes.push(b'\n');
    Ok(bytes)
}
fn protocol_error(error: serde_json::Error) -> Error {
    protocol(error.to_string())
}

/// Run one service as a sequential worker until stdin closes.
///
/// The host kills the process on cancellation/timeout, so expensive commands
/// need not cooperate to keep the shell responsive. Do not spawn descendants
/// or write diagnostics to stdout. Published state belongs to the host, not
/// this process; return replacement snapshots through [`Output`].
pub fn serve(service: &dyn Service) -> Result<(), Error> {
    serve_io(service, std::io::stdin().lock(), std::io::stdout().lock())
}

fn serve_io(
    service: &dyn Service,
    mut input: impl BufRead,
    mut output: impl Write,
) -> Result<(), Error> {
    fn read(input: &mut impl BufRead) -> Result<Option<Frame>, Error> {
        let mut bytes = Vec::new();
        input
            .take((FRAME_BYTES + 2) as u64)
            .read_until(b'\n', &mut bytes)
            .map_err(worker)?;
        if bytes.is_empty() {
            return Ok(None);
        }
        if bytes.len() > FRAME_BYTES + 1 || !bytes.ends_with(b"\n") {
            return Err(protocol("oversized or unterminated frame"));
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(protocol_error)
    }
    fn send(output: &mut impl Write, frame: &Frame) -> Result<(), Error> {
        output.write_all(&encode(frame)?).map_err(worker)?;
        output.flush().map_err(worker)
    }
    match read(&mut input)? {
        Some(Frame::Hello {
            version: PROTOCOL_VERSION,
        }) => {}
        _ => return Err(protocol("expected compatible hello")),
    }
    let descriptor = service.descriptor();
    send(
        &mut output,
        &Frame::Ready {
            version: PROTOCOL_VERSION,
            service: descriptor.name.into(),
            commands: descriptor.commands.iter().map(|s| (*s).into()).collect(),
        },
    )?;
    while let Some(frame) = read(&mut input)? {
        let response = match frame {
            Frame::Command { id, method, args } => {
                let result = (|| {
                    if !descriptor.commands.contains(&method.as_str()) {
                        return Err(Error::new("command", "unknown command"));
                    }
                    crate::validate(&args, crate::MAX_COMMAND_BYTES)?;
                    let result = service.command(&method, args, &Cancellation::default())?;
                    crate::validate(&result.data, crate::MAX_COMMAND_BYTES)?;
                    if let Some(snapshot) = &result.snapshot {
                        crate::validate(snapshot, crate::MAX_SNAPSHOT_BYTES)?;
                    }
                    Ok(result)
                })();
                match result {
                    Ok(output) => Frame::Success { id, output },
                    Err(error) => Frame::Failure {
                        id,
                        error: clamp(error),
                    },
                }
            }
            Frame::Refresh { id } => {
                let result = (|| {
                    let snapshot = service.refresh()?;
                    if let Some(snapshot) = &snapshot {
                        crate::validate(snapshot, crate::MAX_SNAPSHOT_BYTES)?;
                    }
                    Ok(snapshot)
                })();
                match result {
                    Ok(snapshot) => Frame::Snapshot { id, snapshot },
                    Err(error) => Frame::Failure {
                        id,
                        error: clamp(error),
                    },
                }
            }
            _ => return Err(protocol("expected command or refresh")),
        };
        send(&mut output, &response)?;
    }
    Ok(())
}

/// Bound a worker error before it crosses the wire.
fn clamp(error: Error) -> Error {
    Error::new(
        error.code.chars().take(64).collect::<String>(),
        error.message.chars().take(1024).collect::<String>(),
    )
}

/// Explicit host-side executable registration (one service per process).
///
/// ```
/// use riced_extension_sdk::{Descriptor, Registry, process::ProcessExtension};
/// let mut registry = Registry::default();
/// ProcessExtension {
///     name: "colors",
///     descriptor: Descriptor { name: "colors", commands: &["generate"] },
///     argv: vec!["/path/to/colors-worker".into()],
///     directory: std::env::current_dir()?,
///     timeout: std::time::Duration::from_secs(30),
/// }.register(&mut registry, &[])?;
/// // Registration is inert: no worker has started yet.
/// assert!(!registry.available("colors")?);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct ProcessExtension {
    /// Unique extension identity.
    pub name: &'static str,
    /// Expected service and command capability list; checked during handshake.
    pub descriptor: Descriptor,
    /// Program path followed by arguments; never interpreted by a shell.
    pub argv: Vec<String>,
    /// Absolute working directory for the worker.
    pub directory: PathBuf,
    /// End-to-end deadline including startup, handshake, and queue wait.
    pub timeout: Duration,
}

impl ProcessExtension {
    /// Validate options and register without starting a child or supervisor.
    pub fn register(self, registry: &mut crate::Registry, reserved: &[&str]) -> Result<(), Error> {
        if self.argv.is_empty()
            || self.argv.len() > 64
            || self.argv.iter().any(|s| s.contains('\0') || s.len() > 4096)
            || self.argv[0].is_empty()
            || !self.directory.is_absolute()
            || !(Duration::from_millis(10)..=Duration::from_secs(300)).contains(&self.timeout)
        {
            return Err(Error::new(
                "options",
                "invalid argv, non-absolute directory, or timeout outside 10..300000 ms",
            ));
        }
        let name = self.name;
        let service = Arc::new(ProcessService {
            options: self,
            supervisor: Mutex::new(None),
            shared: Arc::new(Shared {
                available: Arc::new(AtomicBool::new(false)),
                stopping: Arc::new(AtomicBool::new(false)),
                want_refresh: Arc::new(AtomicBool::new(false)),
                pushed: Arc::new(Mutex::new(None)),
            }),
        });
        struct Registration {
            name: &'static str,
            service: Arc<ProcessService>,
        }
        impl Extension for Registration {
            fn manifest(&self) -> Manifest {
                Manifest {
                    name: self.name,
                    sdk_version: SDK_VERSION,
                }
            }
            fn services(&self) -> Vec<Arc<dyn Service>> {
                vec![self.service.clone()]
            }
        }
        registry.register(&Registration { name, service }, reserved)
    }
}

struct Call {
    method: String,
    args: Value,
    cancel: Cancellation,
    deadline: Instant,
    response: mpsc::SyncSender<Result<Output, Error>>,
}
struct Supervisor {
    sender: tokio::sync::mpsc::Sender<Call>,
    thread: JoinHandle<()>,
}
struct ProcessService {
    options: ProcessExtension,
    supervisor: Mutex<Option<Supervisor>>,
    shared: Arc<Shared>,
}

/// State shared between the service handle and its supervisor thread.
struct Shared {
    available: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    /// Set by [`Service::refresh`]; the supervisor polls the worker when idle.
    want_refresh: Arc<AtomicBool>,
    /// Most recent unsolicited snapshot, coalesced until the host reads it.
    pushed: Arc<Mutex<Option<Value>>>,
}

impl Service for ProcessService {
    fn descriptor(&self) -> Descriptor {
        Descriptor {
            name: self.options.descriptor.name,
            commands: self.options.descriptor.commands,
        }
    }
    fn initial_snapshot(&self) -> Result<Value, Error> {
        Ok(serde_json::json!({}))
    }
    fn available(&self) -> bool {
        self.shared.available.load(Ordering::Acquire)
    }
    fn refresh(&self) -> Result<Option<Value>, Error> {
        // Ask the supervisor to poll the worker on its next idle tick, then hand
        // back the most recent pushed snapshot. Never blocks the host thread.
        self.shared.want_refresh.store(true, Ordering::Release);
        Ok(self
            .shared
            .pushed
            .lock()
            .map_err(|_| worker("push lock poisoned"))?
            .take())
    }
    fn shutdown(&self) {
        self.shared.stopping.store(true, Ordering::Release);
        if let Ok(mut supervisor) = self.supervisor.lock()
            && let Some(supervisor) = supervisor.take()
        {
            drop(supervisor.sender);
            let _ = supervisor.thread.join();
        }
    }
    fn command(&self, method: &str, args: Value, cancel: &Cancellation) -> Result<Output, Error> {
        if self.shared.stopping.load(Ordering::Acquire) {
            return Err(worker("service is shutting down"));
        }
        let deadline = Instant::now() + self.options.timeout;
        let (response, receiver) = mpsc::sync_channel(1);
        let call = Call {
            method: method.into(),
            args,
            cancel: cancel.clone(),
            deadline,
            response,
        };
        {
            let mut slot = self
                .supervisor
                .lock()
                .map_err(|_| worker("supervisor lock poisoned"))?;
            // Recheck under the lock so shutdown cannot race a fresh supervisor.
            if self.shared.stopping.load(Ordering::Acquire) {
                return Err(worker("service is shutting down"));
            }
            if slot.is_none() {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(worker)?;
                let (sender, receiver) = tokio::sync::mpsc::channel(1);
                let argv = self.options.argv.clone();
                let directory = self.options.directory.clone();
                let descriptor = self.descriptor();
                let shared = self.shared.clone();
                let timeout = self.options.timeout;
                let thread = std::thread::Builder::new()
                    .name(format!("riced-{}", self.options.name))
                    .spawn(move || {
                        runtime.block_on(supervise(
                            receiver, argv, directory, descriptor, shared, timeout,
                        ))
                    })
                    .map_err(worker)?;
                *slot = Some(Supervisor { sender, thread });
            }
            slot.as_ref()
                .ok_or_else(|| worker("missing supervisor"))?
                .sender
                .try_send(call)
                .map_err(|_| Error::new("busy", "process service queue is full or closed"))?;
        }
        // A cancelled queued caller must not hold a host worker until another
        // app's active command finishes. The supervisor independently enforces
        // this same token/deadline and discards expired queued work.
        loop {
            if cancel.is_cancelled() || self.shared.stopping.load(Ordering::Acquire) {
                return Err(Error::new("cancelled", "worker command cancelled"));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::new("timeout", "worker command timed out"));
            }
            match receiver.recv_timeout(remaining.min(Duration::from_millis(10))) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(worker("supervisor disconnected"));
                }
            }
        }
    }
}
impl Drop for ProcessService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Session {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
}
impl Session {
    fn spawn(argv: &[String], directory: &std::path::Path) -> Result<Self, Error> {
        let executable = if argv[0].contains('/') {
            directory.join(&argv[0])
        } else {
            PathBuf::from(&argv[0])
        };
        let mut command = tokio::process::Command::new(executable);
        command
            .args(&argv[1..])
            .current_dir(directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(worker)?;
        let input = child.stdin.take().ok_or_else(|| worker("missing stdin"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| worker("missing stdout"))?;
        Ok(Self {
            child,
            input,
            output: BufReader::new(output),
            next_id: 0,
        })
    }
    async fn send(&mut self, frame: &Frame) -> Result<(), Error> {
        self.input
            .write_all(&encode(frame)?)
            .await
            .map_err(worker)?;
        self.input.flush().await.map_err(worker)
    }
    async fn read(&mut self) -> Result<Frame, Error> {
        let mut bytes = Vec::new();
        loop {
            let buffer = self.output.fill_buf().await.map_err(worker)?;
            if buffer.is_empty() {
                return Err(worker("worker disconnected"));
            }
            let end = buffer.iter().position(|b| *b == b'\n');
            let count = end.map_or(buffer.len(), |n| n + 1);
            if bytes.len() + count > FRAME_BYTES + 1 {
                return Err(protocol("frame exceeds byte limit"));
            }
            bytes.extend_from_slice(&buffer[..count]);
            self.output.consume(count);
            if end.is_some() {
                return serde_json::from_slice(&bytes).map_err(protocol_error);
            }
        }
    }
    async fn handshake(&mut self, descriptor: &Descriptor) -> Result<(), Error> {
        self.send(&Frame::Hello {
            version: PROTOCOL_VERSION,
        })
        .await?;
        match self.read().await? {
            Frame::Ready {
                version: PROTOCOL_VERSION,
                service,
                commands,
            } if service == descriptor.name
                && commands
                    .iter()
                    .map(String::as_str)
                    .eq(descriptor.commands.iter().copied()) =>
            {
                Ok(())
            }
            _ => Err(protocol(
                "worker capabilities/version do not match registration",
            )),
        }
    }
    async fn execute(&mut self, call: &Call) -> Result<Result<Output, Error>, Error> {
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| protocol("request IDs exhausted"))?;
        let expected = self.next_id;
        self.send(&Frame::Command {
            id: expected,
            method: call.method.clone(),
            args: call.args.clone(),
        })
        .await?;
        match self.read().await? {
            Frame::Success { id, output } if id == expected => {
                crate::validate(&output.data, crate::MAX_COMMAND_BYTES)?;
                if let Some(snapshot) = &output.snapshot {
                    crate::validate(snapshot, crate::MAX_SNAPSHOT_BYTES)?;
                }
                Ok(Ok(output))
            }
            Frame::Failure { id, error }
                if id == expected && error.code.len() <= 256 && error.message.len() <= 4096 =>
            {
                Ok(Err(error))
            }
            _ => Err(protocol("invalid response or mismatched request ID")),
        }
    }
    async fn refresh(&mut self) -> Result<Option<Value>, Error> {
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| protocol("request IDs exhausted"))?;
        let expected = self.next_id;
        self.send(&Frame::Refresh { id: expected }).await?;
        match self.read().await? {
            Frame::Snapshot { id, snapshot } if id == expected => {
                if let Some(snapshot) = &snapshot {
                    crate::validate(snapshot, crate::MAX_SNAPSHOT_BYTES)?;
                }
                Ok(snapshot)
            }
            Frame::Failure { id, error }
                if id == expected && error.code.len() <= 256 && error.message.len() <= 4096 =>
            {
                Err(error)
            }
            _ => Err(protocol(
                "invalid refresh response or mismatched request ID",
            )),
        }
    }
    async fn stop(mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

async fn interrupted(cancel: &Cancellation, stopping: &AtomicBool) {
    while !cancel.is_cancelled() && !stopping.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn supervise(
    mut receiver: tokio::sync::mpsc::Receiver<Call>,
    argv: Vec<String>,
    directory: PathBuf,
    descriptor: Descriptor,
    shared: Arc<Shared>,
    timeout: Duration,
) {
    let available = &shared.available;
    let stopping = &shared.stopping;
    let mut session: Option<Session> = None;
    let mut last_failure = None;
    loop {
        let call = tokio::select! {
            call = receiver.recv() => match call { Some(call) => call, None => break },
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                if stopping.load(Ordering::Acquire) { break; }
                if let Some(worker) = &mut session && !matches!(worker.child.try_wait(), Ok(None)) {
                    available.store(false, Ordering::Release);
                    if let Some(worker) = session.take() { worker.stop().await; }
                    last_failure = Some(Instant::now());
                }
                // Poll the worker for unsolicited state only when the host
                // actually read this service since the last idle tick, and only
                // while no command is in flight so frames cannot interleave.
                if shared.want_refresh.swap(false, Ordering::AcqRel) && session.is_some() {
                    let outcome = match &mut session {
                        Some(worker) => {
                            match tokio::time::timeout(timeout, worker.refresh()).await {
                                Ok(result) => result,
                                Err(_) => Err(Error::new("timeout", "worker refresh timed out")),
                            }
                        }
                        None => Ok(None),
                    };
                    match outcome {
                        Ok(Some(snapshot)) => {
                            if let Ok(mut slot) = shared.pushed.lock() {
                                *slot = Some(snapshot);
                            }
                        }
                        Ok(None) => {}
                        Err(_) => {
                            available.store(false, Ordering::Release);
                            if let Some(worker) = session.take() { worker.stop().await; }
                            last_failure = Some(Instant::now());
                        }
                    }
                }
                continue;
            }
        };
        if call.cancel.is_cancelled() || call.deadline <= Instant::now() {
            let code = if call.cancel.is_cancelled() {
                "cancelled"
            } else {
                "timeout"
            };
            let _ = call
                .response
                .send(Err(Error::new(code, "request expired before dispatch")));
            continue;
        }
        let operation = async {
            if session.is_none() {
                // Back off after failures; never replay failed commands automatically.
                if let Some(failed) = last_failure {
                    tokio::time::sleep_until(tokio::time::Instant::from_std(
                        failed + Duration::from_millis(250),
                    ))
                    .await;
                }
                session = Some(Session::spawn(&argv, &directory)?);
                session
                    .as_mut()
                    .ok_or_else(|| worker("missing session"))?
                    .handshake(&descriptor)
                    .await?;
                available.store(true, Ordering::Release);
            }
            session
                .as_mut()
                .ok_or_else(|| worker("missing session"))?
                .execute(&call)
                .await
        };
        let result = tokio::select! {
            biased;
            _ = interrupted(&call.cancel, stopping) => Err(Error::new("cancelled", "worker command cancelled")),
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(call.deadline)) => Err(Error::new("timeout", "worker command timed out")),
            result = operation => result,
        };
        let response = match result {
            Ok(result) => result,
            Err(error) => {
                available.store(false, Ordering::Release);
                if let Some(worker) = session.take() {
                    worker.stop().await;
                }
                last_failure = Some(Instant::now());
                Err(error)
            }
        };
        let _ = call.response.send(response);
        if stopping.load(Ordering::Acquire) {
            break;
        }
    }
    available.store(false, Ordering::Release);
    if let Some(worker) = session {
        worker.stop().await;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;

    const READY: &str = "read -r hello; printf '%s\\n' '{\"type\":\"ready\",\"version\":2,\"service\":\"test\",\"commands\":[\"run\"]}';";
    fn registry(script: &str, timeout: Duration) -> crate::Registry {
        let mut registry = crate::Registry::default();
        ProcessExtension {
            name: "test",
            descriptor: Descriptor {
                name: "test",
                commands: &["run"],
            },
            argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
            directory: std::env::current_dir().unwrap(),
            timeout,
        }
        .register(&mut registry, &[])
        .unwrap();
        registry
    }
    fn request(registry: &crate::Registry, cancel: &Cancellation) -> Result<Output, Error> {
        registry
            .request("test", "run", json!({}))
            .unwrap()
            .execute(cancel)
    }
    fn wait_available(registry: &crate::Registry, expected: bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while registry.available("test").unwrap() != expected {
            assert!(
                Instant::now() < deadline,
                "availability did not become {expected}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn reuses_worker_and_shutdown_reaps_the_child() {
        let registry = registry(
            &format!(
                "{READY} id=0; while read -r command; do id=$((id+1)); printf '{{\"type\":\"success\",\"id\":%s,\"output\":{{\"data\":%s,\"snapshot\":null}}}}\\n' \"$id\" \"$$\"; done"
            ),
            Duration::from_secs(3),
        );
        let pid = request(&registry, &Cancellation::default()).unwrap().data;
        assert_eq!(
            request(&registry, &Cancellation::default()).unwrap().data,
            pid
        );
        registry.shutdown();
        wait_available(&registry, false);
        #[cfg(target_os = "linux")]
        assert!(!std::path::Path::new(&format!("/proc/{}", pid.as_u64().unwrap())).exists());
    }

    #[test]
    fn crash_does_not_publish_and_next_request_restarts() {
        let registry = registry(
            &format!(
                "{READY} read -r command; printf '{{\"type\":\"success\",\"id\":1,\"output\":{{\"data\":%s,\"snapshot\":null}}}}\\n' \"$$\"; exit 9"
            ),
            Duration::from_secs(3),
        );
        let first = request(&registry, &Cancellation::default()).unwrap().data;
        wait_available(&registry, false);
        let second = request(&registry, &Cancellation::default()).unwrap().data;
        assert_ne!(first, second);
        assert_eq!(registry.snapshot("test").unwrap(), json!({}));
    }

    #[test]
    fn cancellation_kills_unresponsive_worker_and_later_calls_restart() {
        let registry = Arc::new(registry(
            &format!("{READY} read -r command; read -r forever"),
            Duration::from_secs(3),
        ));
        let cancel = Cancellation::default();
        let task = {
            let registry = registry.clone();
            let cancel = cancel.clone();
            std::thread::spawn(move || request(&registry, &cancel))
        };
        wait_available(&registry, true);
        cancel.cancel();
        assert_eq!(task.join().unwrap().err().unwrap().code, "cancelled");
        wait_available(&registry, false);
        let cancel = Cancellation::default();
        let task = {
            let registry = registry.clone();
            let cancel = cancel.clone();
            std::thread::spawn(move || request(&registry, &cancel))
        };
        wait_available(&registry, true);
        cancel.cancel();
        assert_eq!(task.join().unwrap().err().unwrap().code, "cancelled");
    }

    #[test]
    fn timeout_covers_handshake_and_blocked_pipe_writes() {
        let registry = registry("read -r hello; read -r forever", Duration::from_millis(50));
        assert_eq!(
            request(&registry, &Cancellation::default())
                .err()
                .unwrap()
                .code,
            "timeout"
        );
        let registry = self::registry(
            &format!("{READY} while :; do :; done"),
            Duration::from_millis(50),
        );
        let data = json!("x".repeat(60_000));
        assert_eq!(
            registry
                .request("test", "run", data)
                .unwrap()
                .execute(&Cancellation::default())
                .err()
                .unwrap()
                .code,
            "timeout"
        );
    }

    #[test]
    fn oversized_worker_frames_fail_without_unbounded_buffering() {
        let registry = registry(
            "read -r hello; printf '%1200000s\\n' x",
            Duration::from_secs(3),
        );
        assert_eq!(
            request(&registry, &Cancellation::default())
                .err()
                .unwrap()
                .code,
            "protocol"
        );
        assert!(!registry.available("test").unwrap());
    }

    #[test]
    fn saturated_queue_rejects_extra_requests_and_shutdown_interrupts_active_work() {
        let registry = Arc::new(registry(
            &format!("{READY} read -r command; read -r forever"),
            Duration::from_secs(5),
        ));
        let cancel = Cancellation::default();
        let active = {
            let registry = registry.clone();
            let cancel = cancel.clone();
            std::thread::spawn(move || request(&registry, &cancel))
        };
        wait_available(&registry, true);
        let (sender, receiver) = mpsc::channel();
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let registry = registry.clone();
            let sender = sender.clone();
            tasks.push(std::thread::spawn(move || {
                let error = request(&registry, &Cancellation::default()).err().unwrap();
                sender.send(error.code).unwrap();
            }));
        }
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(3)).unwrap(),
            "busy"
        );
        registry.shutdown();
        assert_eq!(active.join().unwrap().err().unwrap().code, "cancelled");
        for task in tasks {
            task.join().unwrap();
        }
        assert!(!registry.available("test").unwrap());
    }

    #[test]
    fn cancelling_a_queued_call_does_not_wait_for_or_kill_another_owners_worker() {
        let registry = Arc::new(registry(
            &format!("{READY} read -r command; read -r forever"),
            Duration::from_secs(5),
        ));
        let active_cancel = Cancellation::default();
        let active = {
            let registry = registry.clone();
            let cancel = active_cancel.clone();
            std::thread::spawn(move || request(&registry, &cancel))
        };
        wait_available(&registry, true);
        let queued_cancel = Cancellation::default();
        let (sender, receiver) = mpsc::channel();
        let queued = {
            let registry = registry.clone();
            let cancel = queued_cancel.clone();
            std::thread::spawn(move || {
                sender
                    .send(request(&registry, &cancel).err().unwrap().code)
                    .unwrap()
            })
        };
        std::thread::sleep(Duration::from_millis(20));
        queued_cancel.cancel();
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap(),
            "cancelled"
        );
        assert!(registry.available("test").unwrap());
        active_cancel.cancel();
        active.join().unwrap().err().unwrap();
        queued.join().unwrap();
        registry.shutdown();
    }

    #[test]
    fn rejects_mismatched_ids_versions_and_malformed_frames() {
        for script in [
            format!("{READY} read -r command; printf '%s\\n' '{{\"type\":\"success\",\"id\":42,\"output\":{{\"data\":null,\"snapshot\":null}}}}'"),
            "read -r hello; printf '%s\\n' '{\"type\":\"ready\",\"version\":99,\"service\":\"test\",\"commands\":[\"run\"]}'".into(),
            "read -r hello; printf 'not-json\\n'".into(),
        ] {
            let registry = registry(&script, Duration::from_secs(3));
            assert_eq!(request(&registry, &Cancellation::default()).err().unwrap().code, "protocol");
            assert!(!registry.available("test").unwrap());
        }
    }

    #[test]
    fn worker_helper_negotiates_and_rejects_invalid_commands() {
        struct Echo;
        impl Service for Echo {
            fn descriptor(&self) -> Descriptor {
                Descriptor {
                    name: "test",
                    commands: &["run"],
                }
            }
            fn initial_snapshot(&self) -> Result<Value, Error> {
                Ok(Value::Null)
            }
            fn command(&self, _: &str, args: Value, _: &Cancellation) -> Result<Output, Error> {
                Ok(Output {
                    data: args,
                    snapshot: None,
                })
            }
        }
        let mut output = Vec::new();
        let input = b"{\"type\":\"hello\",\"version\":2}\n{\"type\":\"command\",\"id\":7,\"method\":\"run\",\"args\":42}\n{\"type\":\"command\",\"id\":8,\"method\":\"unknown\",\"args\":null}\n";
        serve_io(&Echo, &input[..], &mut output).unwrap();
        let lines: Vec<Value> = output
            .split(|b| *b == b'\n')
            .filter(|s| !s.is_empty())
            .map(|s| serde_json::from_slice(s).unwrap())
            .collect();
        assert_eq!(lines[0]["type"], "ready");
        assert_eq!(lines[1]["output"]["data"], 42);
        assert_eq!(lines[2]["error"]["code"], "command");
        let input = vec![b'x'; FRAME_BYTES + 2];
        assert_eq!(
            serve_io(&Echo, input.as_slice(), Vec::new())
                .unwrap_err()
                .code,
            "protocol"
        );
    }

    #[test]
    fn worker_helper_answers_refresh_with_snapshots_and_errors() {
        struct Ticker;
        impl Service for Ticker {
            fn descriptor(&self) -> Descriptor {
                Descriptor {
                    name: "test",
                    commands: &["run"],
                }
            }
            fn initial_snapshot(&self) -> Result<Value, Error> {
                Ok(Value::Null)
            }
            fn refresh(&self) -> Result<Option<Value>, Error> {
                Ok(Some(json!({"tick": 1})))
            }
            fn command(&self, _: &str, _: Value, _: &Cancellation) -> Result<Output, Error> {
                Err(Error::new("boom", "nope"))
            }
        }
        let input =
            b"{\"type\":\"hello\",\"version\":2}\n{\"type\":\"refresh\",\"id\":5}\n{\"type\":\"command\",\"id\":6,\"method\":\"run\",\"args\":null}\n";
        let mut output = Vec::new();
        serve_io(&Ticker, &input[..], &mut output).unwrap();
        let lines: Vec<Value> = output
            .split(|b| *b == b'\n')
            .filter(|s| !s.is_empty())
            .map(|s| serde_json::from_slice(s).unwrap())
            .collect();
        assert_eq!(lines[0]["type"], "ready");
        assert_eq!(lines[1]["type"], "snapshot");
        assert_eq!(lines[1]["id"], 5);
        assert_eq!(lines[1]["snapshot"]["tick"], 1);
        // A refresh failure is reported like any other failure.
        assert_eq!(lines[2]["type"], "failure");
        assert_eq!(lines[2]["error"]["code"], "boom");
    }

    #[test]
    fn unsolicited_worker_snapshots_are_polled_only_after_a_read() {
        let registry = registry(
            &format!(
                "{READY} id=0; while read -r line; do id=$((id+1)); case \"$line\" in *refresh*) printf '{{\"type\":\"snapshot\",\"id\":%s,\"snapshot\":{{\"tick\":%s}}}}\\n' \"$id\" \"$id\" ;; *) printf '{{\"type\":\"success\",\"id\":%s,\"output\":{{\"data\":null,\"snapshot\":null}}}}\\n' \"$id\" ;; esac; done"
            ),
            Duration::from_secs(3),
        );
        // No worker starts for a poll alone; nothing is pushed yet.
        assert_eq!(registry.refresh().unwrap(), Vec::<String>::new());
        assert!(!registry.available("test").unwrap());
        request(&registry, &Cancellation::default()).unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        let mut published = None;
        while Instant::now() < deadline {
            if !registry.refresh().unwrap().is_empty() {
                published = Some(registry.snapshot("test").unwrap());
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let published = published.expect("no unsolicited snapshot arrived");
        assert!(published["tick"].as_u64().unwrap() >= 1);
        registry.shutdown();
    }
}
