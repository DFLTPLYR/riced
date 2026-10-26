use riced_extension_sdk::*;
use serde_json::{Value, json};
use std::sync::Arc;

struct Example {
    name: &'static str,
    version: u32,
    topics: Vec<&'static str>,
}
struct Echo(&'static str);
impl Extension for Example {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: self.name,
            sdk_version: self.version,
        }
    }
    fn services(&self) -> Vec<Arc<dyn Service>> {
        self.topics
            .iter()
            .map(|name| Arc::new(Echo(name)) as Arc<dyn Service>)
            .collect()
    }
}
impl Service for Echo {
    fn descriptor(&self) -> Descriptor {
        Descriptor {
            name: self.0,
            commands: &["echo"],
        }
    }
    fn initial_snapshot(&self) -> Result<Value, Error> {
        Ok(json!({"count":0}))
    }
    fn command(&self, _: &str, args: Value, _: &Cancellation) -> Result<Output, Error> {
        Ok(Output {
            snapshot: Some(args.clone()),
            data: args,
        })
    }
}
fn example(topics: Vec<&'static str>) -> Example {
    Example {
        name: "example",
        version: SDK_VERSION,
        topics,
    }
}

#[test]
fn incompatible_sdk_is_rejected_before_registration() {
    let mut registry = Registry::default();
    let ext = Example {
        version: SDK_VERSION + 1,
        ..example(vec!["echo"])
    };
    assert_eq!(registry.register(&ext, &[]).unwrap_err().code, "version");
}
#[test]
fn reserved_name_failure_does_not_partially_register_services() {
    let mut registry = Registry::default();
    assert!(
        registry
            .register(&example(vec!["echo", "theme"]), &["theme"])
            .is_err()
    );
    assert!(!registry.contains("echo"));
    registry
        .register(&example(vec!["echo"]), &["theme"])
        .unwrap();
}
#[test]
fn duplicate_topics_are_rejected_atomically() {
    let mut registry = Registry::default();
    assert!(
        registry
            .register(&example(vec!["echo", "echo"]), &[])
            .is_err()
    );
    assert!(!registry.contains("echo"));
}
#[test]
fn duplicate_extension_identity_is_rejected() {
    let mut registry = Registry::default();
    registry.register(&example(vec!["echo"]), &[]).unwrap();
    assert!(registry.register(&example(vec!["another"]), &[]).is_err());
}
#[test]
fn invalid_topic_identifiers_are_rejected() {
    let mut registry = Registry::default();
    assert!(registry.register(&example(vec!["bad.topic"]), &[]).is_err());
}
#[test]
fn execution_does_not_publish_until_the_host_commits() {
    let mut registry = Registry::default();
    registry.register(&example(vec!["echo"]), &[]).unwrap();
    let output = registry
        .request("echo", "echo", json!({"count":1}))
        .unwrap()
        .execute(&Cancellation::default())
        .unwrap();
    assert_eq!(registry.snapshot("echo").unwrap(), json!({"count":0}));
    registry.publish("echo", output.snapshot.unwrap()).unwrap();
    assert_eq!(registry.snapshot("echo").unwrap(), json!({"count":1}));
}
#[test]
fn detached_snapshots_cannot_mutate_the_registry() {
    let mut registry = Registry::default();
    registry.register(&example(vec!["echo"]), &[]).unwrap();
    let mut snapshot = registry.snapshot("echo").unwrap();
    snapshot["count"] = json!(5);
    assert_eq!(registry.snapshot("echo").unwrap()["count"], 0);
}
#[test]
fn unknown_commands_and_services_fail_before_execution() {
    let mut registry = Registry::default();
    registry.register(&example(vec!["echo"]), &[]).unwrap();
    assert_eq!(
        registry
            .request("echo", "missing", Value::Null)
            .unwrap_err()
            .code,
        "command"
    );
    assert_eq!(
        registry
            .request("missing", "echo", Value::Null)
            .unwrap_err()
            .code,
        "service"
    );
}
#[test]
fn cancelled_requests_never_execute() {
    let mut registry = Registry::default();
    registry.register(&example(vec!["echo"]), &[]).unwrap();
    let cancel = Cancellation::default();
    cancel.cancel();
    let result = registry
        .request("echo", "echo", Value::Null)
        .unwrap()
        .execute(&cancel);
    assert_eq!(result.err().unwrap().code, "cancelled");
}
#[test]
fn byte_depth_and_node_limits_are_enforced() {
    assert!(validate(&json!("x".repeat(MAX_COMMAND_BYTES)), MAX_COMMAND_BYTES).is_err());
    let mut deep = Value::Null;
    for _ in 0..34 {
        deep = json!([deep]);
    }
    assert!(validate(&deep, MAX_COMMAND_BYTES).is_err());
    assert!(validate(&json!(vec![0; 16_384]), MAX_SNAPSHOT_BYTES).is_err());
}
#[test]
fn invalid_results_and_snapshots_are_rejected_without_publication() {
    let mut registry = Registry::default();
    registry.register(&example(vec!["echo"]), &[]).unwrap();
    assert!(
        registry
            .publish("echo", json!("x".repeat(MAX_SNAPSHOT_BYTES)))
            .is_err()
    );
    assert!(
        registry
            .request("echo", "echo", json!("x".repeat(MAX_COMMAND_BYTES)))
            .is_err()
    );
    assert_eq!(registry.snapshot("echo").unwrap(), json!({"count":0}));
}

struct Oversized;
impl Extension for Oversized {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "oversized",
            sdk_version: SDK_VERSION,
        }
    }
    fn services(&self) -> Vec<Arc<dyn Service>> {
        vec![Arc::new(Oversized)]
    }
}
impl Service for Oversized {
    fn descriptor(&self) -> Descriptor {
        Descriptor {
            name: "oversized",
            commands: &["data", "snapshot", "cancel"],
        }
    }
    fn initial_snapshot(&self) -> Result<Value, Error> {
        Ok(Value::Null)
    }
    fn command(&self, method: &str, _: Value, cancel: &Cancellation) -> Result<Output, Error> {
        Ok(match method {
            "data" => Output {
                data: json!("x".repeat(MAX_COMMAND_BYTES)),
                snapshot: None,
            },
            "snapshot" => Output {
                data: Value::Null,
                snapshot: Some(json!("x".repeat(MAX_SNAPSHOT_BYTES))),
            },
            "cancel" => {
                cancel.cancel();
                Output {
                    data: Value::Null,
                    snapshot: None,
                }
            }
            _ => unreachable!(),
        })
    }
}
#[test]
fn oversized_worker_results_are_rejected() {
    let mut registry = Registry::default();
    registry.register(&Oversized, &[]).unwrap();
    for method in ["data", "snapshot"] {
        let result = registry
            .request("oversized", method, Value::Null)
            .unwrap()
            .execute(&Cancellation::default());
        assert_eq!(result.err().unwrap().code, "limit");
    }
    assert_eq!(registry.snapshot("oversized").unwrap(), Value::Null);
}
#[test]
fn cancellation_during_execution_discards_the_result() {
    let mut registry = Registry::default();
    registry.register(&Oversized, &[]).unwrap();
    let result = registry
        .request("oversized", "cancel", Value::Null)
        .unwrap()
        .execute(&Cancellation::default());
    assert_eq!(result.err().unwrap().code, "cancelled");
}
