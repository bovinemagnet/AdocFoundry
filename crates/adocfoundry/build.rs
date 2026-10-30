//! Embeds the third-party notices of the release build for Help › About, generated from Cargo's
//! metadata by the module `cargo xtask notices` uses, and names the build. Embeds the app icon
//! in the Windows executable, as resource 1, where GPUI looks for the window's icon and Explorer
//! for the file's.

use std::{env, fs, path::Path};

// `cargo xtask notices` uses the rest of it.
#[allow(dead_code)]
#[path = "../../xtask/src/notices.rs"]
mod notices;

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
    let root = Path::new(&manifest).join("../..");
    let out = env::var("OUT_DIR").unwrap();
    write_notices(&root, Path::new(&out));
    println!(
        "cargo:rustc-env=ADOCFOUNDRY_BUILD={}, {}",
        env::var("TARGET").unwrap(),
        env::var("PROFILE").unwrap()
    );

    let icon = root.join("packaging/icons/adocfoundry.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let rc = Path::new(&out).join("adocfoundry.rc");
    let icon = icon.display().to_string().replace('\\', "/");
    fs::write(&rc, format!("1 ICON \"{icon}\"\n")).unwrap();
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}

/// Writes `notices.json` to `out`. A dependency without a licence text fails the build.
fn write_notices(root: &Path, out: &Path) {
    for path in [
        "Cargo.lock",
        "Cargo.toml",
        "xtask/src/notices.rs",
        "xtask/licences",
    ] {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    for asset in notices::ASSETS {
        for file in asset.notices {
            let path = root.join(asset.folder).join(file);
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    let cargo = env::var_os("CARGO").unwrap();
    let notices = notices::collect(root, &cargo)
        .unwrap_or_else(|error| panic!("third-party notices: {error}"));
    fs::write(out.join("notices.json"), notices.to_json()).unwrap();
}
