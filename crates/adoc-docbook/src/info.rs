//! The root's `info`: the title and subtitle, and only the metadata the document's attributes
//! give, as Asciidoctor's DocBook converter writes it. Nothing is made up: an attribute that
//! is not set writes nothing.
//!
//! - `author`, `author_2`, … (with `firstname`, `middlename`, `lastname` and `email`, and their
//!   numbered forms, as a header's author line sets them) are `author`s; else `authors`, names
//!   separated by semicolons (or commas when there is no semicolon), each a `personname`.
//! - `revdate` is the `date`; with `revnumber` or `revremark` as well, a `revhistory` of one
//!   `revision`. A `revnumber` with no date, which a `revision` cannot hold, is the
//!   `releaseinfo`.
//! - `keywords`, separated by commas, are a `keywordset`; `description` is the `abstract`.

use std::collections::BTreeMap;

use crate::document::{Title, Writer};

impl Writer<'_> {
    pub(crate) fn info(
        &mut self,
        title: Title,
        subtitle: Option<&str>,
        attributes: &BTreeMap<String, String>,
    ) {
        let attribute = |name: &str| {
            attributes
                .get(name)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
        };
        self.xml.open("info", &[]);
        self.title(&title);
        if let Some(subtitle) = subtitle.filter(|subtitle| !subtitle.trim().is_empty()) {
            self.xml.element("subtitle", &[], subtitle.trim());
        }
        for author in authors(attributes) {
            self.xml.open("author", &[]);
            self.xml.open("personname", &[]);
            if author.first.is_some() || author.last.is_some() {
                for (name, part) in [
                    ("firstname", &author.first),
                    ("othername", &author.middle),
                    ("surname", &author.last),
                ] {
                    if let Some(part) = part {
                        self.xml.element(name, &[], part);
                    }
                }
            } else {
                self.xml.text(&author.name);
            }
            self.xml.close("personname");
            if let Some(email) = &author.email {
                self.xml.element("email", &[], email);
            }
            self.xml.close("author");
        }
        let date = attribute("revdate");
        let number = attribute("revnumber");
        let remark = attribute("revremark");
        if let Some(date) = date {
            self.xml.element("date", &[], date);
        }
        match date {
            Some(date) if number.is_some() || remark.is_some() => {
                self.xml.open("revhistory", &[]);
                self.xml.open("revision", &[]);
                if let Some(number) = number {
                    self.xml.element("revnumber", &[], number);
                }
                self.xml.element("date", &[], date);
                if let Some(remark) = remark {
                    self.xml.element("revremark", &[], remark);
                }
                self.xml.close("revision");
                self.xml.close("revhistory");
            }
            None => {
                if let Some(number) = number {
                    self.xml.element("releaseinfo", &[], number);
                }
            }
            Some(_) => {}
        }
        let keywords: Vec<&str> = attribute("keywords")
            .map(|keywords| {
                keywords
                    .split(',')
                    .map(str::trim)
                    .filter(|keyword| !keyword.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        if !keywords.is_empty() {
            self.xml.open("keywordset", &[]);
            for keyword in keywords {
                self.xml.element("keyword", &[], keyword);
            }
            self.xml.close("keywordset");
        }
        if let Some(description) = attribute("description") {
            self.xml.open("abstract", &[]);
            self.xml.element("para", &[], description);
            self.xml.close("abstract");
        }
        self.xml.close("info");
        self.xml.newline();
    }
}

/// One author, as the attributes name them.
#[derive(Debug, Default, PartialEq)]
struct Author {
    name: String,
    first: Option<String>,
    middle: Option<String>,
    last: Option<String>,
    email: Option<String>,
}

/// The authors the attributes name, in order.
fn authors(attributes: &BTreeMap<String, String>) -> Vec<Author> {
    let attribute = |name: &str| {
        attributes
            .get(name)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    let numbered = |name: &str, index: usize| {
        if index == 1 {
            attribute(name)
        } else {
            attribute(&format!("{name}_{index}"))
        }
    };
    if attribute("author").is_some() {
        let count = attribute("authorcount")
            .and_then(|count| count.parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(1, 1000);
        return (1..=count)
            .filter_map(|index| {
                Some(Author {
                    name: numbered("author", index)?,
                    first: numbered("firstname", index),
                    middle: numbered("middlename", index),
                    last: numbered("lastname", index),
                    email: numbered("email", index),
                })
            })
            .collect();
    }
    let Some(names) = attribute("authors") else {
        return Vec::new();
    };
    let separator = if names.contains(';') { ';' } else { ',' };
    names
        .split(separator)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| Author {
            name: name.to_owned(),
            ..Author::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attributes(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn authors_come_from_the_header_or_a_list() {
        let header = authors(&attributes(&[
            ("author", "Jane Doe"),
            ("firstname", "Jane"),
            ("lastname", "Doe"),
            ("email", "jane@example.com"),
            ("author_2", "John Roe"),
            ("authorcount", "2"),
        ]));
        assert_eq!(header.len(), 2);
        assert_eq!(header[0].first.as_deref(), Some("Jane"));
        assert_eq!(header[0].email.as_deref(), Some("jane@example.com"));
        assert_eq!(header[1].name, "John Roe");
        assert_eq!(header[1].email, None);

        let listed = authors(&attributes(&[("authors", "Ann Lee; Bo, Jr")]));
        let names: Vec<&str> = listed.iter().map(|author| author.name.as_str()).collect();
        assert_eq!(names, ["Ann Lee", "Bo, Jr"]);
        let listed = authors(&attributes(&[("authors", "Ann, Bo")]));
        assert_eq!(listed.len(), 2);
        assert!(authors(&attributes(&[("authors", " ")])).is_empty());
    }
}
