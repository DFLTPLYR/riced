//! Owned task descriptions and cancellation. No Lua values enter async work.
use iced::futures::future::{Either, select, try_join3};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap, os::unix::process::CommandExt, path::PathBuf, process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::oneshot,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ProcessOptions {
    pub timeout_ms: u64,
    pub max_output_bytes: usize,
    pub cwd: Option<PathBuf>,
}
impl Default for ProcessOptions {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            max_output_bytes: 65_536,
            cwd: None,
        }
    }
}
#[derive(Debug, Clone)]
pub(crate) enum Operation {
    PickFile(super::dialog::FileOptions),
    File(super::storage::Request),
    Process {
        argv: Vec<String>,
        options: ProcessOptions,
    },
    Timer {
        millis: u64,
        repeat: bool,
    },
}
#[derive(Debug)]
pub(crate) enum Command {
    Start {
        id: u64,
        operation: Operation,
    },
    PollFile {
        id: u64,
        revision: u64,
        request: super::storage::Request,
    },
    Cancel(u64),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryKind {
    Job,
    Service,
}
#[derive(Debug, Clone)]
pub struct Delivery {
    pub kind: DeliveryKind,
    pub generation: u64,
    pub id: u64,
    /// Service subscriptions use revisions to discard superseded snapshots.
    pub revision: u64,
    pub payload: Value,
}
#[derive(Debug)]
struct Running {
    operation: Operation,
    cancel: Option<oneshot::Sender<()>>,
    revision: u64,
}
#[derive(Debug, Default)]
pub(crate) struct Owner {
    generation: u64,
    active: HashMap<u64, Running>,
}
impl Owner {
    pub fn reset(&mut self, generation: u64) {
        if self.generation != generation {
            self.active.clear();
            self.generation = generation;
        }
    }
    pub fn cancel_all(&mut self) {
        self.active.clear();
    }
    pub fn apply(&mut self, commands: Vec<Command>) {
        for command in commands {
            match command {
                Command::PollFile {
                    id,
                    revision,
                    request,
                } => {
                    self.active.insert(
                        id,
                        Running {
                            operation: Operation::File(request),
                            cancel: None,
                            revision: revision.saturating_sub(1),
                        },
                    );
                }
                Command::Start { id, operation } => {
                    self.active.insert(
                        id,
                        Running {
                            operation,
                            cancel: None,
                            revision: 0,
                        },
                    );
                }
                Command::Cancel(id) => {
                    self.active.remove(&id);
                }
            }
        }
    }
    pub fn jobs(&mut self) -> Vec<Job> {
        self.active
            .iter_mut()
            .filter_map(|(&id, running)| {
                if running.cancel.is_some() {
                    return None;
                }
                let (cancel, receiver) = oneshot::channel();
                running.cancel = Some(cancel);
                running.revision += 1;
                Some(Job {
                    generation: self.generation,
                    id,
                    revision: running.revision,
                    operation: running.operation.clone(),
                    receiver,
                })
            })
            .collect()
    }
    pub fn complete(&mut self, delivery: &Delivery) -> bool {
        if self.generation != delivery.generation
            || !self
                .active
                .get(&delivery.id)
                .is_some_and(|r| r.cancel.is_some() && r.revision == delivery.revision)
        {
            return false;
        }
        let repeat = self
            .active
            .get(&delivery.id)
            .is_some_and(|r| matches!(r.operation, Operation::Timer { repeat: true, .. }));
        if repeat {
            if let Some(running) = self.active.get_mut(&delivery.id) {
                running.cancel = None;
            }
        } else {
            self.active.remove(&delivery.id);
        }
        true
    }
}
pub(crate) struct Job {
    pub generation: u64,
    pub id: u64,
    revision: u64,
    operation: Operation,
    receiver: oneshot::Receiver<()>,
}
impl Job {
    pub async fn run(mut self) -> Delivery {
        let cancelled = !matches!(
            self.receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        );
        let payload = if cancelled {
            failure("cancelled", "operation cancelled before starting")
        } else {
            match self.operation {
                Operation::PickFile(options) => super::dialog::run(options, self.receiver).await,
                Operation::File(request) => super::storage::run(request, self.receiver).await,
                Operation::Timer { millis, .. } => match select(
                    Box::pin(tokio::time::sleep(Duration::from_millis(millis))),
                    self.receiver,
                )
                .await
                {
                    Either::Left(_) => json!({"ok":true,"type":"timer"}),
                    Either::Right(_) => failure("cancelled", "timer cancelled"),
                },
                Operation::Process { argv, options } => process(argv, options, self.receiver).await,
            }
        };
        Delivery {
            kind: DeliveryKind::Job,
            generation: self.generation,
            id: self.id,
            revision: self.revision,
            payload,
        }
    }
}
#[derive(Debug, Serialize)]
struct TaskError {
    code: &'static str,
    message: String,
}
fn failure(code: &'static str, message: impl Into<String>) -> Value {
    json!({"ok":false,"error":TaskError { code, message:message.into() }})
}
async fn read_output(reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, TaskError> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| TaskError {
            code: "io",
            message: e.to_string(),
        })?;
    if bytes.len() > limit {
        return Err(TaskError {
            code: "output_limit",
            message: "process output exceeded its byte limit".into(),
        });
    }
    Ok(bytes)
}
async fn process(
    argv: Vec<String>,
    options: ProcessOptions,
    receiver: oneshot::Receiver<()>,
) -> Value {
    let Some(program) = argv.first() else {
        return failure("spawn", "empty command");
    };
    let mut command = tokio::process::Command::new(program);
    command.as_std_mut().process_group(0);
    command
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = options.cwd {
        command.current_dir(cwd);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return failure("spawn", e.to_string()),
    };
    let mut group = ProcessGroup(
        child
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(rustix::process::Pid::from_raw),
    );
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill().await;
        return failure("io", "process output pipes unavailable");
    };
    let result = {
        let output = try_join3(
            read_output(stdout, options.max_output_bytes),
            read_output(stderr, options.max_output_bytes),
            async {
                child.wait().await.map_err(|e| TaskError {
                    code: "io",
                    message: e.to_string(),
                })
            },
        );
        let stop = select(
            Box::pin(tokio::time::sleep(Duration::from_millis(
                options.timeout_ms,
            ))),
            receiver,
        );
        match select(Box::pin(output), Box::pin(stop)).await {
            Either::Left((result, _)) => result,
            Either::Right((Either::Left(_), _)) => Err(TaskError {
                code: "timeout",
                message: "process timed out".into(),
            }),
            Either::Right((Either::Right(_), _)) => Err(TaskError {
                code: "cancelled",
                message: "process cancelled".into(),
            }),
        }
    };
    match result {
        Ok((stdout, stderr, status)) => {
            group.0 = None;
            let mut result = json!({"ok":status.success(),"type":"process","code":status.code(),"stdout":String::from_utf8_lossy(&stdout),"stderr":String::from_utf8_lossy(&stderr)});
            if !status.success() {
                result["error"] = json!(TaskError {
                    code: "exit",
                    message: status.to_string()
                });
            }
            result
        }
        Err(error) => {
            group.terminate();
            let _ = child.kill().await;
            json!({"ok":false,"type":"process","error":error})
        }
    }
}
struct ProcessGroup(Option<rustix::process::Pid>);
impl ProcessGroup {
    fn terminate(&mut self) {
        if let Some(pid) = self
            .0
            .take()
            .filter(|pid| *pid != rustix::process::Pid::INIT)
        {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn execute(operation: Operation) -> Value {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut owner = Owner::default();
                owner.reset(1);
                owner.apply(vec![Command::Start { id: 1, operation }]);
                owner.jobs().pop().unwrap().run().await.payload
            })
    }
    #[test]
    fn process_captures_output_and_nonzero_exit_status() {
        let result = execute(Operation::Process {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf out; printf err >&2; exit 7".into(),
            ],
            options: Default::default(),
        });
        assert_eq!(result["stdout"], "out");
        assert_eq!(result["stderr"], "err");
        assert_eq!(result["code"], 7);
        assert_eq!(result["ok"], false);
    }
    #[test]
    fn processes_are_bounded_by_deadline_and_output_size() {
        let result = execute(Operation::Process {
            argv: vec!["/bin/sh".into(), "-c".into(), "while :; do :; done".into()],
            options: ProcessOptions {
                timeout_ms: 10,
                ..Default::default()
            },
        });
        assert_eq!(result["error"]["code"], "timeout");
        let result = execute(Operation::Process {
            argv: vec!["/bin/sh".into(), "-c".into(), "printf 123456789".into()],
            options: ProcessOptions {
                max_output_bytes: 4,
                ..Default::default()
            },
        });
        assert_eq!(result["error"]["code"], "output_limit");
    }
    #[test]
    fn cancellation_and_generation_changes_reject_late_results() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut owner = Owner::default();
                owner.reset(1);
                owner.apply(vec![Command::Start {
                    id: 1,
                    operation: Operation::Timer {
                        millis: 60_000,
                        repeat: false,
                    },
                }]);
                let job = owner.jobs().pop().unwrap();
                owner.reset(2);
                let result = tokio::time::timeout(Duration::from_secs(1), job.run())
                    .await
                    .unwrap();
                assert_eq!(result.payload["error"]["code"], "cancelled");
                assert!(!owner.complete(&result));
            });
    }
    #[test]
    fn cancelled_picker_jobs_never_open_a_native_dialog() {
        let mut owner = Owner::default();
        owner.reset(1);
        owner.apply(vec![Command::Start {
            id: 1,
            operation: Operation::PickFile(Default::default()),
        }]);
        let job = owner.jobs().pop().unwrap();
        owner.reset(2);
        let result = iced::futures::executor::block_on(job.run());
        assert_eq!(result.payload["error"]["code"], "cancelled");
        assert!(!owner.complete(&result));
    }
    #[test]
    fn recurring_timers_reject_duplicate_ticks_and_stop_when_cancelled() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut owner = Owner::default();
                owner.reset(1);
                owner.apply(vec![Command::Start {
                    id: 1,
                    operation: Operation::Timer {
                        millis: 1,
                        repeat: true,
                    },
                }]);
                let first = owner.jobs().pop().unwrap().run().await;
                assert!(owner.complete(&first));
                assert!(!owner.complete(&first));
                let next = owner.jobs().pop().unwrap();
                assert!(!owner.complete(&first));
                owner.apply(vec![Command::Cancel(1)]);
                let cancelled = next.run().await;
                assert_eq!(cancelled.payload["error"]["code"], "cancelled");
                assert!(!owner.complete(&cancelled));
            });
    }
    #[test]
    fn cancelling_a_running_process_reaps_the_child() {
        let marker = std::path::PathBuf::from("/tmp/opencode")
            .join(format!("riced-managed-process-{}", std::process::id()));
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let mut owner = Owner::default();
                owner.reset(1);
                owner.apply(vec![Command::Start {
                    id: 1,
                    operation: Operation::Process {
                        argv: vec![
                            "/bin/sh".into(),
                            "-c".into(),
                            format!(
                                "printf '%s' \"$$\" > '{}'; while :; do :; done",
                                marker.display()
                            ),
                        ],
                        options: Default::default(),
                    },
                }]);
                let job = tokio::spawn(owner.jobs().pop().unwrap().run());
                let pid = tokio::time::timeout(Duration::from_secs(2), async {
                    loop {
                        if let Ok(contents) = std::fs::read_to_string(&marker)
                            && let Ok(pid) = contents.parse::<i32>()
                        {
                            break pid;
                        }
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                })
                .await
                .unwrap();
                owner.cancel_all();
                let delivery = tokio::time::timeout(Duration::from_secs(2), job)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(delivery.payload["error"]["code"], "cancelled");
                assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
            });
        std::fs::remove_file(marker).unwrap();
    }
}
