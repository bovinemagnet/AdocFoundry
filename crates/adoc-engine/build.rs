//! Embeds the built-in New Antora Project templates (`templates/<id>/`) as data: every file of
//! each template, by its path relative to the template, through `include_str!`.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let templates = manifest.join("templates");
    println!("cargo:rerun-if-changed={}", templates.display());
    let mut ids: Vec<PathBuf> = fs::read_dir(&templates)
        .expect("the templates folder")
        .map(|entry| entry.expect("a template").path())
        .filter(|path| path.is_dir())
        .collect();
    ids.sort();
    let mut code =
        String::from("pub(crate) static BUILTIN_FILES: &[(&str, &[(&str, &str)])] = &[\n");
    for template in &ids {
        let id = template.file_name().unwrap().to_string_lossy().into_owned();
        let mut files = Vec::new();
        collect(template, template, &mut files);
        files.sort();
        code.push_str(&format!("    ({id:?}, &[\n"));
        for (relative, path) in files {
            println!("cargo:rerun-if-changed={}", path.display());
            code.push_str(&format!(
                "        ({relative:?}, include_str!({:?})),\n",
                path.display().to_string()
            ));
        }
        code.push_str("    ]),\n");
    }
    code.push_str("];\n");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("templates.rs");
    fs::write(out, code).expect("write templates.rs");
}

/// Every file under `folder`, by its `/`-separated path relative to `base`.
fn collect(base: &Path, folder: &Path, files: &mut Vec<(String, PathBuf)>) {
    for entry in fs::read_dir(folder).expect("a template folder") {
        let path = entry.expect("a template file").path();
        if path.is_dir() {
            println!("cargo:rerun-if-changed={}", path.display());
            collect(base, &path, files);
        } else {
            let relative = path
                .strip_prefix(base)
                .unwrap()
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            files.push((relative, path));
        }
    }
}
