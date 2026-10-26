//! Explicit compiled-in registration and bounded native command execution.
use iced::futures::future::{Either, select};
use riced_extension_sdk::{Cancellation, Error, Registry, Request};
use serde_json::{Value, json};
use std::sync::OnceLock;
use tokio::sync::{Semaphore, oneshot};

/// Built-in service topics unavailable to extension services.
pub(crate) const BUILTIN_TOPICS: &[&str] = &[
    "system",
    "theme",
    "notifications",
    "wayland",
    "systemtray",
    "bar",
    "riced",
    "iced",
    "size",
    "notification",
    "tray",
];

static REGISTRY: OnceLock<Result<Registry, Error>> = OnceLock::new();
static WORKERS: Semaphore = Semaphore::const_new(4);

/// Install the registry built by a generated or default shell entry point.
///
/// The first installation wins; a generated shell installs before any Lua or
/// service access. Tests fall back to [`default_registry`].
pub(crate) fn install(registry: Registry) {
    let _ = REGISTRY.set(Ok(registry));
}

pub(crate) fn default_registry() -> Result<Registry, Error> {
    let mut registry = Registry::default();
    riced_extension_palette::register(&mut registry, BUILTIN_TOPICS)?;
    riced_extension_theme_gen::register(&mut registry, BUILTIN_TOPICS)?;
    Ok(registry)
}

pub(crate) fn registry() -> Result<&'static Registry, Error> {
    REGISTRY
        .get_or_init(default_registry)
        .as_ref()
        .map_err(Clone::clone)
}

struct CancelOnDrop(Cancellation);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
fn failure(error: Error) -> Value {
    json!({"ok":false,"error":{"code":error.code.chars().take(64).collect::<String>(),"message":error.message.chars().take(1024).collect::<String>()}})
}
pub(crate) async fn run(request: Request, receiver: oneshot::Receiver<()>) -> Value {
    let cancel = CancelOnDrop(Cancellation::default());
    let token = cancel.0.clone();
    let work = async move {
        let permit = WORKERS
            .acquire()
            .await
            .map_err(|e| Error::new("worker", e.to_string()))?;
        // The worker owns the permit even if its awaiting app is cancelled.
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            request.execute(&token)
        })
        .await
        .map_err(|e| Error::new("worker", e.to_string()))?
    };
    match select(Box::pin(work), receiver).await {
        Either::Left((Ok(output), _)) => {
            let mut result = json!({"ok":true,"data":output.data});
            if let Some(snapshot) = output.snapshot {
                result["snapshot"] = snapshot;
            }
            result
        }
        Either::Left((Err(error), _)) => failure(error),
        Either::Right(_) => failure(Error::new("cancelled", "extension command cancelled")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::tasks::{Command, Operation, Owner};

    #[test]
    fn cancelling_running_work_signals_its_token() {
        use riced_extension_sdk::{Descriptor, Extension, Manifest, Output, SDK_VERSION, Service};
        use std::{
            sync::{Arc, mpsc},
            time::{Duration, Instant},
        };
        struct Worker(mpsc::Sender<Cancellation>, mpsc::Sender<()>);
        impl Service for Worker {
            fn descriptor(&self) -> Descriptor {
                Descriptor {
                    name: "worker",
                    commands: &["wait"],
                }
            }
            fn initial_snapshot(&self) -> Result<Value, Error> {
                Ok(Value::Null)
            }
            fn command(&self, _: &str, _: Value, cancel: &Cancellation) -> Result<Output, Error> {
                self.0.send(cancel.clone()).unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                while !cancel.is_cancelled() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(1));
                }
                self.1.send(()).unwrap();
                Ok(Output {
                    data: Value::Null,
                    snapshot: None,
                })
            }
        }
        struct Ext(Arc<Worker>);
        impl Extension for Ext {
            fn manifest(&self) -> Manifest {
                Manifest {
                    name: "worker",
                    sdk_version: SDK_VERSION,
                }
            }
            fn services(&self) -> Vec<Arc<dyn Service>> {
                vec![self.0.clone()]
            }
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (started, receive_started) = mpsc::channel();
                let (finished, receive_finished) = mpsc::channel();
                let mut registry = Registry::default();
                registry
                    .register(&Ext(Arc::new(Worker(started, finished))), &[])
                    .unwrap();
                let request = registry.request("worker", "wait", Value::Null).unwrap();
                let (cancel, receiver) = oneshot::channel();
                let task = tokio::spawn(run(request, receiver));
                let token = tokio::task::spawn_blocking(move || {
                    receive_started
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap()
                })
                .await
                .unwrap();
                cancel.send(()).unwrap();
                let result = task.await.unwrap();
                assert_eq!(result["error"]["code"], "cancelled");
                assert!(token.is_cancelled());
                tokio::task::spawn_blocking(move || {
                    receive_finished
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap()
                })
                .await
                .unwrap();
            });
    }

    #[test]
    fn snapshot_publication_requires_live_owner_acceptance() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let registry = registry().unwrap();
                let mut owner = Owner::default();
                owner.reset(700);
                let operation = || Operation::Extension {
                    service: "palette".into(),
                    request: registry
                        .request("palette", "generate", json!({"seed":"#123abc","dark":true}))
                        .unwrap(),
                };
                let before = registry.snapshot("palette").unwrap();
                owner.apply(vec![Command::Start {
                    id: 1,
                    operation: operation(),
                }]);
                let delivery = owner.jobs().pop().unwrap().run().await;
                assert_eq!(delivery.payload["ok"], true);
                assert_eq!(registry.snapshot("palette").unwrap(), before);
                assert!(owner.complete(&delivery));
                assert_eq!(
                    registry.snapshot("palette").unwrap(),
                    delivery.payload["data"]
                );
                assert!(!owner.complete(&delivery));

                let accepted = registry.snapshot("palette").unwrap();
                owner.apply(vec![Command::Start {
                    id: 2,
                    operation: operation(),
                }]);
                let mut late = owner.jobs().pop().unwrap().run().await;
                late.payload["snapshot"] = json!({"stale":true});
                owner.reset(701);
                assert!(!owner.complete(&late));
                assert_eq!(registry.snapshot("palette").unwrap(), accepted);

                owner.apply(vec![Command::Start {
                    id: 3,
                    operation: operation(),
                }]);
                let job = owner.jobs().pop().unwrap();
                owner.apply(vec![Command::Cancel(3)]);
                let cancelled = job.run().await;
                assert_eq!(cancelled.payload["error"]["code"], "cancelled");
                assert!(!owner.complete(&cancelled));
                assert_eq!(registry.snapshot("palette").unwrap(), accepted);
            });
    }
}
