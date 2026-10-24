//! Embeds the app icon in the Windows executable, as resource 1, where GPUI looks for the
//! window's icon and Explorer for the file's.

use std::{env, fs, path::Path};

fn main() {
    let icon = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("../../packaging/icons/adocfoundry.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let rc = Path::new(&env::var("OUT_DIR").unwrap()).join("adocfoundry.rc");
    let icon = icon.display().to_string().replace('\\', "/");
    fs::write(&rc, format!("1 ICON \"{icon}\"\n")).unwrap();
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
