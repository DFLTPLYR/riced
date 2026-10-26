//! Generate icon lookup exclusively from the checked-in, pinned SVG catalog.
#[path = "scripts/lucide_codegen.rs"]
mod lucide_codegen;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=assets/lucide/icons.json");
    println!("cargo:rerun-if-changed=scripts/lucide_codegen.rs");
    let catalog: serde_json::Value =
        serde_json::from_str(include_str!("assets/lucide/icons.json"))?;
    let icons = serde_json::from_value(catalog["icons"].clone())?;
    let code = lucide_codegen::generate(&icons)?;
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?);
    std::fs::write(out.join("lucide_lookup.rs"), code)?;
    Ok(())
}
