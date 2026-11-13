mod access;
mod diagnostic;
mod document;
mod path;
pub mod probe;
mod reference;
mod symbol;

pub use access::FileAccess;
pub use diagnostic::{Diagnostic, DiagnosticCode, DiagnosticSeverity, Related};
pub use document::{
    AttributeDeclaration, Document, DocumentTitle, LineIndex, Location, SourceRange, UiMacro,
};
pub use path::normalize_path;
pub use reference::{xref_file, ImageDirective, IncludeDirective, Reference, ReferenceKind};
pub use symbol::{
    alphanumeric_id, canonical_id, plain_title, section_ids, section_ids_with_conditionals, Anchor,
    IdStyle, Section,
};

/// The folders a workspace leaves out unless `[workspace] ignore` says otherwise: the index,
/// Antora discovery and the file watcher never enter them, and the Files view hides them.
pub const DEFAULT_IGNORED_DIRECTORIES: &[&str] = &[
    ".cache",
    ".git",
    ".zed",
    "node_modules",
    "build",
    "target",
    "dist",
    ".idea",
    ".gradle",
];

/// The names in [`DEFAULT_IGNORED_DIRECTORIES`] that are build output beside a project, but
/// ordinary module or topic folders inside the `modules` folder of an Antora component
/// version, where nothing is built.
pub const BUILD_OUTPUT_DIRECTORIES: &[&str] = &["build", "target", "dist"];

/// Whether `beneath`, a path relative to the workspace root holding it, lies in a folder that
/// `ignore` leaves out: a folder it names, except a build output folder
/// ([`BUILD_OUTPUT_DIRECTORIES`]) beneath the `modules` folder of a folder for which
/// `is_component` holds (one holding `antora.yml`, given relative to the
/// same root). Every component of `beneath` is taken for a folder. The index, the file
/// watcher and discovery from a revision's files all decide by this.
pub fn is_ignored_beneath(
    beneath: &std::path::Path,
    ignore: &[impl AsRef<str>],
    is_component: impl Fn(&std::path::Path) -> bool,
) -> bool {
    let mut folder = std::path::PathBuf::new();
    // The position of the `modules` folder of a component, once one is passed.
    let mut modules = None;
    for (position, part) in beneath.components().enumerate() {
        let name = part.as_os_str().to_str().unwrap_or_default();
        let inside_modules = modules.is_some_and(|at| position > at);
        if ignore.iter().any(|ignored| ignored.as_ref() == name)
            && !(inside_modules && BUILD_OUTPUT_DIRECTORIES.contains(&name))
        {
            return true;
        }
        if modules.is_none() && name == "modules" && is_component(&folder) {
            modules = Some(position);
        }
        folder.push(part);
    }
    false
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn build_output_names_are_ordinary_folders_inside_modules() {
        let component = |folder: &Path| folder == Path::new("docs");
        let ignored = |path: &str| {
            is_ignored_beneath(Path::new(path), DEFAULT_IGNORED_DIRECTORIES, component)
        };
        assert!(!ignored("docs/modules/ROOT/pages/build"));
        assert!(!ignored("docs/modules/ROOT/pages/dist/target"));
        assert!(ignored("build"), "beside the component");
        assert!(
            ignored("docs/build"),
            "in the component, outside its modules"
        );
        assert!(!ignored("docs/modules/build"), "a module named build");
        assert!(ignored("docs/modules/ROOT/pages/node_modules"));
        assert!(ignored("docs/modules/ROOT/.git"));
        assert!(ignored("other/modules/ROOT/build"), "not a component");
        assert!(ignored("build/docs/modules/ROOT/pages"));
    }
}
