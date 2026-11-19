//! Pictures, diagrams, formulas, audio and video (PRD §5.4).
//!
//! A local picture is read through the options' `FileAccess`, recognised by its bytes (PNG,
//! JPEG, GIF, or SVG whose root is `svg`), and, with [`AssetPolicy::Copy`], handed back as an
//! [`Asset`] named for the order it is first used, `i<n>.<ext>`, each distinct picture once, its
//! `fileref` the assets folder's name and that name. With [`AssetPolicy::Reference`] nothing is
//! copied and `fileref` is the path from the document's folder to the file, which only works
//! where those files stay. A picture that cannot be read, is not a picture, or is too large
//! keeps its description and gets a warning. A remote picture is never fetched: its address is
//! the `fileref`. Diagrams the view has drawn are pictures too; one not drawn keeps its source.
//! A formula is its source as a `mathphrase`, or, where a picture was drawn of it, that picture
//! with the source as its `alt` and text alternative (no MathML is made); audio and video at an address are linked, never copied.

use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    path::{Component, Path, PathBuf},
};

use adoc_render::{DiagramView, FormulaImage, ViewBlock, ViewImage};

use crate::{document::Writer, Asset, AssetPolicy};

/// The most one picture may hold.
const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
/// The most the pictures of one document may hold together.
const MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;

/// The pictures written so far.
#[derive(Default)]
pub(crate) struct Media {
    pub(crate) assets: Vec<Asset>,
    /// Each picture's file, by path, once read: its `fileref`, or why it has none.
    by_path: HashMap<PathBuf, Result<String, String>>,
    /// The assets holding bytes of each hash, to store identical content once.
    by_content: HashMap<u64, Vec<usize>>,
    total: usize,
}

/// What a picture's bytes are, by their first bytes, and the extension its asset takes.
fn kind(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else {
        let text = std::str::from_utf8(bytes).ok()?;
        let document = roxmltree::Document::parse(text).ok()?;
        (document.root_element().tag_name().name() == "svg").then_some("svg")
    }
}

fn hash(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// The path from `from`, a folder, to `to`, both made lexically whole first, written with `/`;
/// `None` when they share no root.
fn relative(from: &Path, to: &Path) -> Option<String> {
    let from: Vec<Component<'_>> = from.components().collect();
    let to_parts: Vec<Component<'_>> = to.components().collect();
    if from.first() != to_parts.first() {
        return None;
    }
    let common = from
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = vec!["..".to_owned(); from.len() - common];
    for part in &to_parts[common..] {
        match part {
            Component::Normal(name) => parts.push(name.to_str()?.to_owned()),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

impl Writer<'_> {
    /// The `fileref` of the local picture at `path`, read and kept, or why it cannot be shown.
    fn local_picture(&mut self, path: &Path) -> Result<String, String> {
        if let Some(known) = self.media.by_path.get(path) {
            return known.clone();
        }
        let result = self.read_picture(path);
        self.media
            .by_path
            .insert(path.to_path_buf(), result.clone());
        result
    }

    fn read_picture(&mut self, path: &Path) -> Result<String, String> {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let bytes = self
            .options
            .access
            .read(path)
            .map_err(|error| format!("{name} could not be read: {error}"))?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(format!(
                "{name} is larger than {} MB",
                MAX_FILE_BYTES / (1024 * 1024)
            ));
        }
        let extension =
            kind(&bytes).ok_or_else(|| format!("{name} is not a PNG, JPEG, GIF or SVG picture"))?;
        match &self.options.assets {
            AssetPolicy::Reference { folder } => relative(folder, path).ok_or_else(|| {
                format!("{name} has no path from the document's folder, so it cannot be referenced")
            }),
            AssetPolicy::Copy { folder } => {
                let key = hash(&bytes);
                if let Some(index) = self.media.by_content.get(&key).and_then(|indices| {
                    indices
                        .iter()
                        .copied()
                        .find(|&index| self.media.assets[index].bytes == bytes)
                }) {
                    return Ok(format!("{folder}/{}", self.media.assets[index].name));
                }
                if self.media.total + bytes.len() > MAX_TOTAL_BYTES {
                    return Err(format!(
                        "{name} would take the document's pictures past {} MB",
                        MAX_TOTAL_BYTES / (1024 * 1024)
                    ));
                }
                let index = self.media.assets.len();
                let asset_name = format!("i{}.{extension}", index + 1);
                self.media.total += bytes.len();
                self.media.assets.push(Asset {
                    name: asset_name.clone(),
                    bytes,
                });
                self.media.by_content.entry(key).or_default().push(index);
                Ok(format!("{folder}/{asset_name}"))
            }
        }
    }

    /// Where a picture is, as a `fileref`, or `None` (with a warning) when it cannot be shown.
    fn picture_ref(&mut self, image: &ViewImage, block: Option<&ViewBlock>) -> Option<String> {
        let span = block
            .map(|block| block.span.clone())
            .or_else(|| self.page.span());
        if let Some(url) = &image.url {
            if url.starts_with("http://") || url.starts_with("https://") {
                self.warn(
                    format!("the picture at {url} is referred to by its address, not copied"),
                    span.as_ref(),
                );
                return Some(url.clone());
            }
        }
        let Some(path) = &image.path else {
            self.warn(
                format!(
                    "the picture {} is not found, so only its description is kept",
                    image.target
                ),
                span.as_ref(),
            );
            return None;
        };
        match self.local_picture(path) {
            Ok(fileref) => Some(fileref),
            Err(why) => {
                self.warn(
                    format!("{why}, so only its description is kept"),
                    span.as_ref(),
                );
                None
            }
        }
    }

    /// A `mediaobject` (or `inlinemediaobject`) of the picture at `fileref`, if any, and its
    /// description as text, so a reader without the picture has its words.
    fn media_object(
        &mut self,
        name: &str,
        fileref: Option<&str>,
        width: Option<&str>,
        height: Option<&str>,
        alt: &str,
    ) {
        self.xml.open(name, &[]);
        if let Some(fileref) = fileref {
            let mut attributes = vec![("fileref", fileref)];
            let width = width.filter(|value| is_length(value));
            let height = height.filter(|value| is_length(value));
            if let Some(width) = width {
                attributes.push(("contentwidth", width));
            }
            if let Some(height) = height {
                attributes.push(("contentdepth", height));
            }
            self.xml.open("imageobject", &[]);
            self.xml.empty("imagedata", &attributes);
            self.xml.close("imageobject");
        }
        if fileref.is_none() || !alt.trim().is_empty() {
            self.xml.open("textobject", &[]);
            self.xml.element("phrase", &[], alt);
            self.xml.close("textobject");
        }
        self.xml.close(name);
    }

    /// A block picture: a `figure` with its title, else an `informalfigure`.
    pub(crate) fn figure(&mut self, block: &ViewBlock, image: &ViewImage, role: &[(&str, &str)]) {
        let fileref = self.picture_ref(image, Some(block));
        self.figure_of(
            block,
            fileref.as_deref(),
            image.width.as_deref(),
            image.height.as_deref(),
            &image.alt,
            role,
        );
    }

    fn figure_of(
        &mut self,
        block: &ViewBlock,
        fileref: Option<&str>,
        width: Option<&str>,
        height: Option<&str>,
        alt: &str,
        role: &[(&str, &str)],
    ) {
        let name = if block.title.is_empty() {
            "informalfigure"
        } else {
            "figure"
        };
        self.xml.open(name, role);
        if !block.title.is_empty() {
            self.xml.open("title", &[]);
            self.inlines(&block.title);
            self.xml.close("title");
        }
        self.media_object("mediaobject", fileref, width, height, alt);
        self.xml.close(name);
    }

    pub(crate) fn inline_picture(&mut self, image: &ViewImage) {
        let fileref = self.picture_ref(image, None);
        self.media_object(
            "inlinemediaobject",
            fileref.as_deref(),
            image.width.as_deref(),
            image.height.as_deref(),
            &image.alt,
        );
    }

    /// A diagram: its picture as a figure when the view drew it, else its source, with a
    /// warning unless the export already said why it was not drawn.
    pub(crate) fn diagram(
        &mut self,
        block: &ViewBlock,
        diagram: &DiagramView,
        role: &[(&str, &str)],
    ) {
        let alt = block
            .attributes
            .get("alt")
            .cloned()
            .unwrap_or_else(|| "A diagram".to_owned());
        match diagram {
            DiagramView::Svg(path) => {
                let fileref = match self.local_picture(path) {
                    Ok(fileref) => Some(fileref),
                    Err(why) => {
                        self.warn(
                            format!(
                                "the diagram's picture: {why}, so only its description is kept"
                            ),
                            Some(&block.span),
                        );
                        None
                    }
                };
                self.figure_of(block, fileref.as_deref(), None, None, &alt, role);
            }
            DiagramView::Pending { kind, source } => {
                self.warn(
                    format!("the {kind} diagram was not drawn in time, so its source is kept"),
                    Some(&block.span),
                );
                self.diagram_source(block, source, role);
            }
            // The export reports why ([`DiagramView::Failed`]'s message) for every format.
            DiagramView::Failed { source, .. } => self.diagram_source(block, source, role),
        }
    }

    fn diagram_source(&mut self, block: &ViewBlock, source: &str, role: &[(&str, &str)]) {
        let attributes: Vec<(&str, &str)> = if role.is_empty() {
            vec![("role", "diagram")]
        } else {
            role.to_vec()
        };
        if !block.title.is_empty() {
            self.xml.open("formalpara", &[]);
            self.xml.open("title", &[]);
            self.inlines(&block.title);
            self.xml.close("title");
            self.xml.open("para", &[]);
        }
        self.xml.element("programlisting", &attributes, source);
        if !block.title.is_empty() {
            self.xml.close("para");
            self.xml.close("formalpara");
        }
    }

    /// A display formula: an `equation` with its title, else an `informalequation`, holding the
    /// picture drawn of it, if any, and its source as a `mathphrase`.
    pub(crate) fn equation(
        &mut self,
        block: &ViewBlock,
        source: &str,
        rendered: &DiagramView,
        role: &[(&str, &str)],
    ) {
        self.page.formulas += 1;
        let name = if block.title.is_empty() {
            "informalequation"
        } else {
            "equation"
        };
        self.xml.open(name, role);
        if !block.title.is_empty() {
            self.xml.open("title", &[]);
            self.inlines(&block.title);
            self.xml.close("title");
        }
        let picture = match rendered {
            DiagramView::Svg(path) => self.local_picture(path).ok(),
            _ => None,
        };
        self.formula(picture.as_deref(), source, "mediaobject");
        self.xml.close(name);
    }

    /// An inline formula: an `inlineequation` holding its picture, if any, and its source.
    pub(crate) fn inline_equation(&mut self, source: &str, rendered: Option<&FormulaImage>) {
        self.page.formulas += 1;
        self.xml.open("inlineequation", &[]);
        let picture = rendered.and_then(|image| self.local_picture(&image.path).ok());
        self.formula(picture.as_deref(), source, "inlinemediaobject");
        self.xml.close("inlineequation");
    }

    /// An equation's content, which the schema allows to be pictures or `mathphrase`s, never
    /// both: with a picture, the source is the equation's `alt` and the picture's text
    /// alternative (a `phrase` with `role="math"`, since a `mathphrase` may not be inside one);
    /// without one, a `mathphrase`.
    fn formula(&mut self, picture: Option<&str>, source: &str, object: &str) {
        let Some(fileref) = picture else {
            self.xml.element("mathphrase", &[], source);
            return;
        };
        self.xml.element("alt", &[], source);
        self.xml.open(object, &[]);
        self.xml.open("imageobject", &[]);
        self.xml.empty("imagedata", &[("fileref", fileref)]);
        self.xml.close("imageobject");
        self.xml.open("textobject", &[]);
        self.xml.element("phrase", &[("role", "math")], source);
        self.xml.close("textobject");
        self.xml.close(object);
    }

    /// Audio or video: each target at an address linked, never copied or fetched; one that is
    /// a file keeps its name, with a warning.
    pub(crate) fn media(
        &mut self,
        block: &ViewBlock,
        video: bool,
        targets: &[String],
        role: &[(&str, &str)],
    ) {
        let (object, data) = if video {
            ("videoobject", "videodata")
        } else {
            ("audioobject", "audiodata")
        };
        let remote: Vec<&String> = targets
            .iter()
            .filter(|target| target.starts_with("http://") || target.starts_with("https://"))
            .collect();
        if remote.len() < targets.len() {
            self.warn(
                "audio and video files are not copied into DocBook, so only their names are kept",
                Some(&block.span),
            );
        }
        let name = if block.title.is_empty() {
            "informalfigure"
        } else {
            "figure"
        };
        self.xml.open(name, role);
        if !block.title.is_empty() {
            self.xml.open("title", &[]);
            self.inlines(&block.title);
            self.xml.close("title");
        }
        self.xml.open("mediaobject", &[]);
        for target in &remote {
            self.xml.open(object, &[]);
            self.xml.empty(data, &[("fileref", target.as_str())]);
            self.xml.close(object);
        }
        self.xml.open("textobject", &[]);
        self.xml.element("phrase", &[], &targets.join(", "));
        self.xml.close("textobject");
        self.xml.close("mediaobject");
        self.xml.close(name);
    }
}

/// Whether `value` is a length DocBook reads: a number with an optional unit (`200`, `50%`,
/// `3in`).
fn is_length(value: &str) -> bool {
    let number = value.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%');
    !number.is_empty()
        && number.parse::<f64>().is_ok_and(|number| number > 0.0)
        && value.len() - number.len() <= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_are_known_by_their_bytes() {
        assert_eq!(kind(b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(kind(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(kind(b"GIF89a..."), Some("gif"));
        assert_eq!(
            kind(b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            Some("svg")
        );
        assert_eq!(kind(b"<html/>"), None);
        assert_eq!(kind(b"not a picture"), None);
    }

    #[test]
    fn paths_are_relative_to_the_document() {
        assert_eq!(
            relative(Path::new("/w/out"), Path::new("/w/docs/a.png")).as_deref(),
            Some("../docs/a.png")
        );
        assert_eq!(
            relative(Path::new("/w"), Path::new("/w/a.png")).as_deref(),
            Some("a.png")
        );
        assert_eq!(relative(Path::new("/w"), Path::new("x/a.png")), None);
    }

    #[test]
    fn lengths_are_numbers_with_units() {
        for value in ["200", "50%", "3in", "1.5cm"] {
            assert!(is_length(value), "{value}");
        }
        for value in ["", "wide", "0", "-3", "3inch"] {
            assert!(!is_length(value), "{value}");
        }
    }
}
