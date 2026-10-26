use riced_extension_sdk::{Cancellation, Descriptor, Registry, process::ProcessExtension};
use serde_json::json;
use std::time::Duration;

#[test]
fn executable_palette_uses_the_same_registry_contract() {
    let mut registry = Registry::default();
    ProcessExtension {
        name: "palette_worker",
        descriptor: Descriptor {
            name: "palette",
            commands: &["generate"],
        },
        argv: vec![env!("CARGO_BIN_EXE_riced-extension-palette-worker").into()],
        directory: std::env::current_dir().unwrap(),
        timeout: Duration::from_secs(5),
    }
    .register(&mut registry, &[])
    .unwrap();
    assert!(!registry.available("palette").unwrap());
    let output = registry
        .request("palette", "generate", json!({"seed":"#89b4fa","dark":true}))
        .unwrap()
        .execute(&Cancellation::default())
        .unwrap();
    assert_eq!(output.data["ready"], true);
    assert_eq!(registry.snapshot("palette").unwrap(), json!({}));
    registry
        .publish("palette", output.snapshot.unwrap())
        .unwrap();
    assert_eq!(registry.snapshot("palette").unwrap(), output.data);
    assert!(registry.available("palette").unwrap());
    let error = registry
        .request("palette", "generate", json!({"seed":"invalid"}))
        .unwrap()
        .execute(&Cancellation::default())
        .err()
        .unwrap();
    assert_eq!(error.code, "seed");
    assert!(registry.available("palette").unwrap());
    registry.shutdown();
    assert!(!registry.available("palette").unwrap());
}
