//! The example worker must satisfy the same registry contract as a compiled-in
//! service, over the SDK's stdio protocol.
use riced_extension_sdk::{Cancellation, Descriptor, Registry, process::ProcessExtension};
use serde_json::json;
use std::time::Duration;

#[test]
fn executable_hello_uses_the_same_registry_contract() {
    let mut registry = Registry::default();
    ProcessExtension {
        name: "hello",
        descriptor: Descriptor {
            name: "hello",
            commands: &["greet"],
        },
        argv: vec![env!("CARGO_BIN_EXE_riced-extension-hello").into()],
        directory: std::env::current_dir().unwrap(),
        timeout: Duration::from_secs(5),
    }
    .register(&mut registry, &[])
    .unwrap();
    assert!(!registry.available("hello").unwrap());
    let output = registry
        .request("hello", "greet", json!({"name": "riced"}))
        .unwrap()
        .execute(&Cancellation::default())
        .unwrap();
    assert_eq!(output.data["greeting"], "hello, riced");
    // The accepted greeting only reaches subscribers after the host publishes
    // the returned snapshot, exactly like a compiled-in service.
    assert_eq!(registry.snapshot("hello").unwrap(), json!({}));
    registry.publish("hello", output.snapshot.unwrap()).unwrap();
    assert_eq!(registry.snapshot("hello").unwrap(), output.data);
    assert!(registry.available("hello").unwrap());
    // A rejected command leaves the cached snapshot and availability intact.
    let error = registry
        .request("hello", "greet", json!({"name": ""}))
        .unwrap()
        .execute(&Cancellation::default())
        .err()
        .unwrap();
    assert_eq!(error.code, "name");
    assert!(registry.available("hello").unwrap());
    registry.shutdown();
    assert!(!registry.available("hello").unwrap());
}
