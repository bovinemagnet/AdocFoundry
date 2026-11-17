//! `word/numbering.xml`: Word numbering for lists. Each kind of list (bullets, or an ordered
//! list's numbering style) has one abstract definition of nine levels; each list is an instance
//! of its own, so an ordered list starts again at its own `start` rather than continuing the
//! last list of the same kind.

use crate::{parts::W, xml::Xml};

/// How a list's items are marked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ListStyle {
    Bullet,
    /// Word's `numFmt` for an ordered list.
    Ordered(&'static str),
}

impl ListStyle {
    /// An ordered list's style as the view gives it: Asciidoctor's names.
    pub(crate) fn ordered(style: Option<&str>) -> Self {
        Self::Ordered(match style {
            Some("loweralpha") => "lowerLetter",
            Some("upperalpha") => "upperLetter",
            Some("lowerroman") => "lowerRoman",
            Some("upperroman") => "upperRoman",
            _ => "decimal",
        })
    }
}

/// The indent of a list item's text at `level`, in twentieths of a point.
pub(crate) fn indent(level: u32) -> u32 {
    720 * (level + 1)
}

/// The deepest level Word numbers.
pub(crate) const DEEPEST_LEVEL: u32 = 8;

#[derive(Default)]
pub(crate) struct Numbering {
    /// The first `abstractNumId` and the `numId` before the first this numbering gives: after a
    /// template's.
    first_abstract: u32,
    last_number: u32,
    abstracts: Vec<ListStyle>,
    /// Each instance: its abstract definition, and the level and number it starts at.
    instances: Vec<(usize, Option<(u32, u32)>)>,
}

impl Numbering {
    /// Numbering whose ids follow a template's, given as its first free `abstractNumId` and
    /// its highest `numId`.
    pub(crate) fn after((first_abstract, last_number): (u32, u32)) -> Self {
        Self {
            first_abstract,
            last_number,
            ..Self::default()
        }
    }

    /// A new list of `style` at `level`, starting at `start` when ordered; its `numId`.
    pub(crate) fn instance(&mut self, style: ListStyle, level: u32, start: u32) -> u32 {
        let index = match self.abstracts.iter().position(|known| *known == style) {
            Some(index) => index,
            None => {
                self.abstracts.push(style);
                self.abstracts.len() - 1
            }
        };
        let restart = matches!(style, ListStyle::Ordered(_)).then_some((level, start));
        self.instances.push((index, restart));
        u32::try_from(self.instances.len())
            .unwrap_or(u32::MAX)
            .saturating_add(self.last_number)
    }

    /// The part, when any list was written.
    pub(crate) fn part(&self) -> Option<Vec<u8>> {
        if self.instances.is_empty() {
            return None;
        }
        let mut xml = Xml::part();
        xml.open("w:numbering", &[("xmlns:w", W)]);
        for (index, style) in self.abstracts.iter().enumerate() {
            let index = u32::try_from(index)
                .unwrap_or(u32::MAX)
                .saturating_add(self.first_abstract);
            xml.open("w:abstractNum", &[("w:abstractNumId", &index.to_string())]);
            xml.empty("w:multiLevelType", &[("w:val", "hybridMultilevel")]);
            for level in 0..=DEEPEST_LEVEL {
                xml.open("w:lvl", &[("w:ilvl", &level.to_string())]);
                xml.empty("w:start", &[("w:val", "1")]);
                let text = match style {
                    ListStyle::Bullet => {
                        xml.empty("w:numFmt", &[("w:val", "bullet")]);
                        ["•", "◦", "▪"][level as usize % 3].to_owned()
                    }
                    ListStyle::Ordered(format) => {
                        xml.empty("w:numFmt", &[("w:val", format)]);
                        format!("%{}.", level + 1)
                    }
                };
                xml.empty("w:lvlText", &[("w:val", &text)]);
                xml.empty("w:lvlJc", &[("w:val", "left")]);
                xml.open("w:pPr", &[]);
                xml.empty(
                    "w:ind",
                    &[("w:left", &indent(level).to_string()), ("w:hanging", "360")],
                );
                xml.close("w:pPr");
                xml.close("w:lvl");
            }
            xml.close("w:abstractNum");
        }
        for (number, (index, restart)) in self.instances.iter().enumerate() {
            let number = u32::try_from(number + 1)
                .unwrap_or(u32::MAX)
                .saturating_add(self.last_number);
            let index = u32::try_from(*index)
                .unwrap_or(u32::MAX)
                .saturating_add(self.first_abstract);
            xml.open("w:num", &[("w:numId", &number.to_string())]);
            xml.empty("w:abstractNumId", &[("w:val", &index.to_string())]);
            if let Some((level, start)) = restart {
                xml.open("w:lvlOverride", &[("w:ilvl", &level.to_string())]);
                xml.empty("w:startOverride", &[("w:val", &start.to_string())]);
                xml.close("w:lvlOverride");
            }
            xml.close("w:num");
        }
        xml.close("w:numbering");
        Some(xml.into_bytes())
    }
}
