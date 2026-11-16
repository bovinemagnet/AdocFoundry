//! A template's styles and numbering with the export's put into them (the Word looks PRD §5.3).
//! The template's parts are kept as they are, byte for byte, apart from the edits made here: the
//! export's styles that the template has no style for, by id or by name, added after its own;
//! what `[docx.styles]` wrote laid over the template's styles; and the export's list numbering
//! added with ids after the template's.

use std::collections::BTreeMap;

use roxmltree::{Document, Node};

use crate::{parts::W, xml::Xml, StyleLook};

/// The order of a run's properties in the schema (`CT_RPr`).
const RUN_ORDER: &[&str] = &[
    "rStyle",
    "rFonts",
    "b",
    "bCs",
    "i",
    "iCs",
    "caps",
    "smallCaps",
    "strike",
    "dstrike",
    "outline",
    "shadow",
    "emboss",
    "imprint",
    "noProof",
    "snapToGrid",
    "vanish",
    "webHidden",
    "color",
    "spacing",
    "w",
    "kern",
    "position",
    "sz",
    "szCs",
    "highlight",
    "u",
    "effect",
    "bdr",
    "shd",
    "fitText",
    "vertAlign",
    "rtl",
    "cs",
    "em",
    "lang",
    "eastAsianLayout",
    "specVanish",
    "oMath",
];

/// What follows a style's run properties in the schema (`CT_Style`).
const AFTER_RUN: &[&str] = &["tblPr", "trPr", "tcPr", "tblStylePr"];

fn is(node: Node<'_, '_>, name: &str) -> bool {
    node.is_element() && node.tag_name().namespace() == Some(W) && node.tag_name().name() == name
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|child| is(*child, name))
}

fn w_attribute<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attribute((W, name))
}

fn val<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(node, name).and_then(|child| w_attribute(child, "val"))
}

/// A part's styles: by id, and ids by name ignoring case, and the default paragraph style.
struct Styles<'a, 'input> {
    by_id: BTreeMap<&'a str, Node<'a, 'input>>,
    by_name: BTreeMap<String, &'a str>,
    default_paragraph: Option<&'a str>,
}

fn styles<'a, 'input>(document: &'a Document<'input>) -> Styles<'a, 'input> {
    let mut found = Styles {
        by_id: BTreeMap::new(),
        by_name: BTreeMap::new(),
        default_paragraph: None,
    };
    for style in document
        .root_element()
        .children()
        .filter(|node| is(*node, "style"))
    {
        let Some(id) = w_attribute(style, "styleId") else {
            continue;
        };
        found.by_id.insert(id, style);
        if let Some(name) = val(style, "name") {
            found.by_name.entry(name.to_lowercase()).or_insert(id);
        }
        if w_attribute(style, "type") == Some("paragraph")
            && matches!(w_attribute(style, "default"), Some("1" | "true" | "on"))
        {
            found.default_paragraph = Some(id);
        }
    }
    found
}

/// The ids the export's styles take in a template: each of `ours` (the export's `styles.xml`)
/// that the template has under another id with the same name, mapped to that id. A style the
/// template has under the same id needs no entry.
pub(crate) fn style_ids(template: &str, ours: &str) -> BTreeMap<String, String> {
    let (Ok(template), Ok(ours)) = (Document::parse(template), Document::parse(ours)) else {
        return BTreeMap::new();
    };
    let theirs = styles(&template);
    let mut renamed = BTreeMap::new();
    for (id, style) in &styles(&ours).by_id {
        if theirs.by_id.contains_key(id) {
            continue;
        }
        let by_name = val(*style, "name")
            .and_then(|name| theirs.by_name.get(&name.to_lowercase()))
            .copied()
            // The default paragraph style, whatever it is called there.
            .or_else(|| {
                (*id == "Normal")
                    .then_some(theirs.default_paragraph)
                    .flatten()
            });
        if let Some(theirs) = by_name {
            renamed.insert((*id).to_owned(), theirs.to_owned());
        }
    }
    renamed
}

/// Applies `edits` (ranges of `text` and what replaces them) to `text`.
fn apply(text: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|(start, end, _)| (*start, *end));
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end, replacement) in edits {
        if start < at {
            continue;
        }
        out.push_str(&text[at..start]);
        out.push_str(&replacement);
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

/// A run property's element as text.
fn element(name: &str, attributes: &[(&str, &str)]) -> String {
    let mut xml = Xml::fragment();
    xml.empty(name, attributes);
    String::from_utf8(xml.into_bytes()).unwrap_or_default()
}

/// The run properties `look` sets, as `(local name, text)`.
fn run_properties(look: &StyleLook) -> Vec<(&'static str, String)> {
    let mut properties = Vec::new();
    if let Some(font) = &look.font {
        properties.push((
            "rFonts",
            element(
                "w:rFonts",
                &[
                    ("w:ascii", font),
                    ("w:hAnsi", font),
                    ("w:eastAsia", font),
                    ("w:cs", font),
                ],
            ),
        ));
    }
    if let Some(colour) = &look.colour {
        properties.push(("color", element("w:color", &[("w:val", colour)])));
    }
    if let Some(size) = look.size {
        let size = size.to_string();
        properties.push(("sz", element("w:sz", &[("w:val", &size)])));
        properties.push(("szCs", element("w:szCs", &[("w:val", &size)])));
    }
    properties
}

/// The edit that lays `look`'s run properties over those of `parent` (a style, or
/// `w:rPrDefault`): its `w:rPr` rewritten in the schema's order, or one added where the schema
/// puts it.
fn restyle(text: &str, parent: Node<'_, '_>, look: &StyleLook) -> Option<(usize, usize, String)> {
    let new = run_properties(look);
    if new.is_empty() {
        return None;
    }
    let position = |name: &str| {
        RUN_ORDER
            .iter()
            .position(|known| *known == name)
            .unwrap_or(RUN_ORDER.len())
    };
    match child(parent, "rPr") {
        Some(run) => {
            let mut properties: Vec<(usize, String)> = run
                .children()
                .filter(|node| node.is_element())
                .filter(|node| {
                    node.tag_name().namespace() != Some(W)
                        || !new.iter().any(|(name, _)| *name == node.tag_name().name())
                })
                .map(|node| {
                    let order = if node.tag_name().namespace() == Some(W) {
                        position(node.tag_name().name())
                    } else {
                        RUN_ORDER.len()
                    };
                    (order, text[node.range()].to_owned())
                })
                .collect();
            properties.extend(new.into_iter().map(|(name, xml)| (position(name), xml)));
            properties.sort_by_key(|(order, _)| *order);
            let inside: String = properties.into_iter().map(|(_, xml)| xml).collect();
            let range = run.range();
            Some((range.start, range.end, format!("<w:rPr>{inside}</w:rPr>")))
        }
        None => {
            let inside: String = new.into_iter().map(|(_, xml)| xml).collect();
            let rpr = format!("<w:rPr>{inside}</w:rPr>");
            let at = match parent
                .children()
                .find(|node| AFTER_RUN.iter().any(|name| is(*node, name)))
            {
                Some(next) => next.range().start,
                None => end_tag(text, parent)?,
            };
            Some((at, at, rpr))
        }
    }
}

/// Where `node`'s end tag starts; `None` for an empty element.
fn end_tag(text: &str, node: Node<'_, '_>) -> Option<usize> {
    let range = node.range();
    let source = &text[range.clone()];
    if source.ends_with("/>") {
        return None;
    }
    source.rfind("</").map(|at| range.start + at)
}

/// The template's `styles.xml` with the export's put into it. `ours` is the export's own
/// `styles.xml`; `looks` gives, for each of the export's style ids, what `[docx.styles]` wrote
/// for it, and `body` what it wrote for the document's defaults.
pub(crate) fn styles_part(
    template: &str,
    ours: &str,
    looks: impl Fn(&str) -> StyleLook,
    body: &StyleLook,
) -> Vec<u8> {
    let (Ok(theirs_document), Ok(ours_document)) =
        (Document::parse(template), Document::parse(ours))
    else {
        return template.as_bytes().to_vec();
    };
    let theirs = styles(&theirs_document);
    let ours_styles = styles(&ours_document);
    let renamed = style_ids(template, ours);
    let mut edits = Vec::new();

    // What `[docx.styles]` wrote, over the template's styles and defaults.
    for id in ours_styles.by_id.keys() {
        let target = renamed.get(*id).map_or(*id, String::as_str);
        if let Some(style) = theirs.by_id.get(target) {
            let mut look = looks(id);
            if Some(target) == theirs.default_paragraph {
                look = look.with(body);
            }
            edits.extend(restyle(template, *style, &look));
        }
    }
    let root = theirs_document.root_element();
    let defaults = child(root, "docDefaults");
    match defaults.and_then(|defaults| child(defaults, "rPrDefault")) {
        Some(run) => edits.extend(restyle(template, run, body)),
        None => {
            let inside: String = run_properties(body)
                .into_iter()
                .map(|(_, xml)| xml)
                .collect();
            if !inside.is_empty() {
                let run = format!("<w:rPrDefault><w:rPr>{inside}</w:rPr></w:rPrDefault>");
                match defaults {
                    Some(defaults) => {
                        if let Some(first) = defaults.children().find(|node| node.is_element()) {
                            let at = first.range().start;
                            edits.push((at, at, run));
                        } else if let Some(at) = end_tag(template, defaults) {
                            edits.push((at, at, run));
                        }
                    }
                    None => {
                        if let Some(first) = root.children().find(|node| node.is_element()) {
                            let at = first.range().start;
                            edits.push((at, at, format!("<w:docDefaults>{run}</w:docDefaults>")));
                        }
                    }
                }
            }
        }
    }

    // The export's styles the template has none for, based on its styles.
    let mut added = String::new();
    for (id, style) in &ours_styles.by_id {
        if theirs.by_id.contains_key(id) || renamed.contains_key(*id) {
            continue;
        }
        let source = &ours[style.range()];
        let mut style_edits = Vec::new();
        for reference in ["basedOn", "next", "link"] {
            let Some(node) = child(*style, reference) else {
                continue;
            };
            let Some(value) = w_attribute(node, "val") else {
                continue;
            };
            if let Some(theirs) = renamed.get(value) {
                let range = node.range();
                style_edits.push((
                    range.start - style.range().start,
                    range.end - style.range().start,
                    element(&format!("w:{reference}"), &[("w:val", theirs)]),
                ));
            }
        }
        let text = apply(source, style_edits).replacen(" w:default=\"1\"", "", 1);
        added.push_str(&text);
    }
    if let Some(at) = end_tag(template, root) {
        edits.push((at, at, added));
    }
    apply(template, edits).into_bytes()
}

/// A template's font table with its embedded faces taken out, naming its families only.
pub(crate) fn without_embeds(table: &[u8]) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(table) else {
        return table.to_vec();
    };
    let Ok(document) = Document::parse(text) else {
        return table.to_vec();
    };
    let edits = document
        .descendants()
        .filter(|node| {
            node.is_element()
                && node.tag_name().namespace() == Some(W)
                && node.tag_name().name().starts_with("embed")
        })
        .map(|node| (node.range().start, node.range().end, String::new()))
        .collect();
    apply(text, edits).into_bytes()
}

/// The highest `abstractNumId` and `numId` the template's numbering uses.
pub(crate) fn numbering_ids(template: &str) -> (u32, u32) {
    let Ok(document) = Document::parse(template) else {
        return (0, 0);
    };
    let highest = |name: &str, attribute: &str| {
        document
            .root_element()
            .children()
            .filter(|node| is(*node, name))
            .filter_map(|node| w_attribute(node, attribute)?.parse::<u32>().ok())
            .max()
    };
    (
        highest("abstractNum", "abstractNumId").map_or(0, |id| id + 1),
        highest("num", "numId").unwrap_or(0),
    )
}

/// The template's `numbering.xml` with the export's definitions and instances (`ours`, written
/// with ids after the template's) put after its own, each where the schema puts it.
pub(crate) fn numbering_part(template: &str, ours: Option<&[u8]>) -> Vec<u8> {
    let Some(ours) = ours.and_then(|ours| std::str::from_utf8(ours).ok()) else {
        return template.as_bytes().to_vec();
    };
    let (Ok(theirs), Ok(ours_document)) = (Document::parse(template), Document::parse(ours)) else {
        return template.as_bytes().to_vec();
    };
    let collect = |name: &str| -> String {
        ours_document
            .root_element()
            .children()
            .filter(|node| is(*node, name))
            .map(|node| &ours[node.range()])
            .collect()
    };
    let (abstracts, numbers) = (collect("abstractNum"), collect("num"));
    let root = theirs.root_element();
    let last = |name: &str| {
        root.children()
            .rev()
            .find(|node| is(*node, name))
            .map(|node| node.range().end)
    };
    let first = |name: &str| {
        root.children()
            .find(|node| is(*node, name))
            .map(|node| node.range().start)
    };
    // An empty template numbering holds nothing to keep.
    let Some(end) = end_tag(template, root) else {
        return ours.as_bytes().to_vec();
    };
    let cleanup = first("numIdMacAtCleanup");
    let abstract_at = last("abstractNum")
        .or_else(|| first("num"))
        .or(cleanup)
        .unwrap_or(end);
    let number_at = last("num").or(cleanup).unwrap_or(end);
    apply(
        template,
        vec![
            (abstract_at, abstract_at, abstracts),
            (number_at, number_at, numbers),
        ],
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = r#"<?xml version="1.0"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#;

    #[test]
    fn styles_the_template_has_by_name_are_its_own() {
        let template = format!(
            r#"{HEAD}<w:style w:type="paragraph" w:default="1" w:styleId="Standard"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="berschrift1"><w:name w:val="heading 1"/></w:style></w:styles>"#
        );
        let ours = format!(
            r#"{HEAD}<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="SourceCode"><w:name w:val="Source Code"/><w:basedOn w:val="Normal"/><w:next w:val="Heading1"/></w:style></w:styles>"#
        );
        let ids = style_ids(&template, &ours);
        assert_eq!(ids["Normal"], "Standard");
        assert_eq!(ids["Heading1"], "berschrift1");
        let merged = String::from_utf8(styles_part(
            &template,
            &ours,
            |_| StyleLook::default(),
            &StyleLook::default(),
        ))
        .unwrap();
        roxmltree::Document::parse(&merged).unwrap();
        // Only the style it lacks is added, based on its own styles.
        assert_eq!(merged.matches("w:styleId=").count(), 3, "{merged}");
        assert!(merged.contains(
            r#"<w:style w:type="paragraph" w:styleId="SourceCode"><w:name w:val="Source Code"/><w:basedOn w:val="Standard"/><w:next w:val="berschrift1"/></w:style></w:styles>"#
        ), "{merged}");
    }

    #[test]
    fn written_styles_change_the_templates_and_keep_the_schemas_order() {
        let template = format!(
            r#"{HEAD}<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Arial"/><w:lang w:val="de-DE"/></w:rPr></w:rPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:rPr><w:b/><w:sz w:val="40"/><w:color w:val="111111"/></w:rPr></w:style></w:styles>"#
        );
        let ours = format!(
            r#"{HEAD}<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style></w:styles>"#
        );
        let heading = StyleLook {
            font: Some("Brand".to_owned()),
            colour: Some("FF0000".to_owned()),
            ..StyleLook::default()
        };
        let body = StyleLook {
            size: Some(24),
            ..StyleLook::default()
        };
        let merged = String::from_utf8(styles_part(
            &template,
            &ours,
            |id| {
                if id == "Heading1" {
                    heading.clone()
                } else {
                    StyleLook::default()
                }
            },
            &body,
        ))
        .unwrap();
        roxmltree::Document::parse(&merged).unwrap();
        assert!(merged.contains(
            r#"<w:rPr><w:rFonts w:ascii="Brand" w:hAnsi="Brand" w:eastAsia="Brand" w:cs="Brand"/><w:b/><w:color w:val="FF0000"/><w:sz w:val="40"/></w:rPr>"#
        ), "{merged}");
        assert!(merged.contains(
            r#"<w:rPr><w:rFonts w:ascii="Arial"/><w:sz w:val="24"/><w:szCs w:val="24"/><w:lang w:val="de-DE"/></w:rPr>"#
        ), "{merged}");
        // The default paragraph style gets the body's too.
        assert!(merged.contains(
            r#"<w:name w:val="Normal"/><w:rPr><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr></w:style>"#
        ), "{merged}");
    }

    #[test]
    fn numbering_follows_the_templates() {
        let w = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
        let template = format!(
            r#"<w:numbering {w}><w:abstractNum w:abstractNumId="3"/><w:num w:numId="7"><w:abstractNumId w:val="3"/></w:num><w:numIdMacAtCleanup w:val="2"/></w:numbering>"#
        );
        assert_eq!(numbering_ids(&template), (4, 7));
        let ours = format!(
            r#"<w:numbering {w}><w:abstractNum w:abstractNumId="4"/><w:num w:numId="8"><w:abstractNumId w:val="4"/></w:num></w:numbering>"#
        );
        let merged = String::from_utf8(numbering_part(&template, Some(ours.as_bytes()))).unwrap();
        assert_eq!(
            merged,
            format!(
                r#"<w:numbering {w}><w:abstractNum w:abstractNumId="3"/><w:abstractNum w:abstractNumId="4"/><w:num w:numId="7"><w:abstractNumId w:val="3"/></w:num><w:num w:numId="8"><w:abstractNumId w:val="4"/></w:num><w:numIdMacAtCleanup w:val="2"/></w:numbering>"#
            )
        );
        assert_eq!(numbering_ids(&format!("<w:numbering {w}/>")), (0, 0));
    }
}
