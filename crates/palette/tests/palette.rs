use riced_extension_palette::Palette;
use riced_extension_sdk::{Cancellation, Registry};
use serde_json::json;

#[test]
fn generates_deterministic_material_colors_without_publishing() {
    let mut registry = Registry::default();
    registry.register(&Palette, &[]).unwrap();
    let generate = || {
        registry
            .request("palette", "generate", json!({"seed":"#AABBCC","dark":true}))
            .unwrap()
            .execute(&Cancellation::default())
            .unwrap()
    };
    let first = generate().data;
    assert_eq!(first, generate().data);
    assert_eq!(first["seed"], "#aabbcc");
    assert_eq!(first["colors"]["primary"].as_str().unwrap().len(), 7);
    assert_eq!(
        registry.snapshot("palette").unwrap(),
        json!({"ready":false})
    );
}
#[test]
fn rejects_invalid_seeds_and_unknown_options() {
    let mut registry = Registry::default();
    registry.register(&Palette, &[]).unwrap();
    for args in [
        json!({"seed":"red"}),
        json!({"seed":"#123"}),
        json!({"seed":"#zzzzzz"}),
        json!({"seed":"#aabbcc","typo":true}),
    ] {
        assert!(
            registry
                .request("palette", "generate", args)
                .unwrap()
                .execute(&Cancellation::default())
                .is_err()
        );
    }
}
