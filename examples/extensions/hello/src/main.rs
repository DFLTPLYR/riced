//! Minimal supervised process extension.
//!
//! One `hello` service with a `greet` command, served over the SDK's stdio
//! protocol. Build it with `riced extension build hello`, then restart the
//! shell to pick up the new executable. See `examples/README.md`.
#![deny(missing_docs)]
use riced_extension_sdk::{
    Cancellation, Descriptor, Error, Extension, Manifest, Output, SDK_VERSION, Service,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// Greeting service. Stateless: the accepted greeting is the snapshot.
pub struct Hello;

impl Extension for Hello {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "hello",
            sdk_version: SDK_VERSION,
        }
    }
    fn services(&self) -> Vec<Arc<dyn Service>> {
        vec![Arc::new(Hello)]
    }
}

/// Arguments for `greet`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Greet {
    #[serde(default = "default_name")]
    name: String,
}

fn default_name() -> String {
    "world".into()
}

impl Service for Hello {
    fn descriptor(&self) -> Descriptor {
        Descriptor {
            name: "hello",
            commands: &["greet"],
        }
    }

    fn initial_snapshot(&self) -> Result<Value, Error> {
        Ok(json!({"greeting": "hello, world"}))
    }

    fn command(&self, method: &str, args: Value, cancel: &Cancellation) -> Result<Output, Error> {
        if method != "greet" {
            return Err(Error::new("command", "unknown hello command"));
        }
        let options: Greet = serde_json::from_value(args)
            .map_err(|error| Error::new("options", error.to_string()))?;
        let name = options.name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err(Error::new("name", "name must be 1..=64 bytes"));
        }
        if cancel.is_cancelled() {
            return Err(Error::new("cancelled", "greeting cancelled"));
        }
        let greeting = format!("hello, {name}");
        Ok(Output {
            data: json!({"greeting": greeting}),
            snapshot: Some(json!({"greeting": greeting})),
        })
    }
}

fn main() -> Result<(), riced_extension_sdk::Error> {
    riced_extension_sdk::process::serve(&Hello)
}
