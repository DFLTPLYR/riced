//! The same palette service, isolated behind the SDK's stdio protocol.
fn main() -> Result<(), riced_extension_sdk::Error> {
    riced_extension_sdk::process::serve(&riced_extension_palette::Palette)
}
