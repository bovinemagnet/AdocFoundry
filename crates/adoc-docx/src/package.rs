//! The OPC package: its parts, each part's relationships, and `[Content_Types].xml`, written
//! in a fixed order so the same parts give the same bytes.

use std::io::Write;

use crate::{xml::Xml, zip::ZipWriter, DocxError};

const CONTENT_TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const RELATIONSHIPS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

pub(crate) const RELATIONSHIPS_TYPE: &str =
    "application/vnd.openxmlformats-package.relationships+xml";

/// A relationship from a part (or the package) to another part.
pub(crate) struct Relationship {
    pub(crate) id: String,
    pub(crate) kind: &'static str,
    /// The target, relative to the folder of the part the relationship is from.
    pub(crate) target: String,
}

/// A part: its name in the package (no leading `/`), content type, bytes and relationships.
pub(crate) struct Part {
    pub(crate) name: String,
    pub(crate) content_type: &'static str,
    pub(crate) data: Vec<u8>,
    pub(crate) relationships: Vec<Relationship>,
}

/// The relationships part of the part named `name`, or of the package for `""`.
fn relationships_name(name: &str) -> String {
    match name.rsplit_once('/') {
        Some((folder, file)) => format!("{folder}/_rels/{file}.rels"),
        None => format!("_rels/{name}.rels"),
    }
}

fn relationships_part(relationships: &[Relationship]) -> Vec<u8> {
    let mut xml = Xml::part();
    xml.open("Relationships", &[("xmlns", RELATIONSHIPS)]);
    for relationship in relationships {
        xml.empty(
            "Relationship",
            &[
                ("Id", &relationship.id),
                ("Type", relationship.kind),
                ("Target", &relationship.target),
            ],
        );
    }
    xml.close("Relationships");
    xml.into_bytes()
}

/// Writes the package to `sink`: `[Content_Types].xml`, the package's relationships, then each
/// part in order, followed by its relationships.
pub(crate) fn write<W: Write>(
    package_relationships: &[Relationship],
    parts: &[Part],
    sink: W,
) -> Result<W, DocxError> {
    let mut types = Xml::part();
    types.open("Types", &[("xmlns", CONTENT_TYPES)]);
    types.empty(
        "Default",
        &[("Extension", "rels"), ("ContentType", RELATIONSHIPS_TYPE)],
    );
    types.empty(
        "Default",
        &[("Extension", "xml"), ("ContentType", "application/xml")],
    );
    for part in parts {
        types.empty(
            "Override",
            &[
                ("PartName", &format!("/{}", part.name)),
                ("ContentType", part.content_type),
            ],
        );
    }
    types.close("Types");

    let mut zip = ZipWriter::new(sink);
    zip.add("[Content_Types].xml", &types.into_bytes())?;
    zip.add(
        &relationships_name(""),
        &relationships_part(package_relationships),
    )?;
    for part in parts {
        zip.add(&part.name, &part.data)?;
        if !part.relationships.is_empty() {
            zip.add(
                &relationships_name(&part.name),
                &relationships_part(&part.relationships),
            )?;
        }
    }
    zip.finish()
}
