//! YAML read into a value that keeps mapping order: a theme's keys are evaluated in the order
//! they are written, and a variable may only refer to a key defined before it.

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Yaml {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Seq(Vec<Yaml>),
    Map(Vec<(String, Yaml)>),
}

impl Yaml {
    /// A mapping key as text, the way Ruby's YAML hands a scalar key to the theme loader.
    fn key(self) -> String {
        match self {
            Self::Null => String::new(),
            Self::Bool(value) => value.to_string(),
            Self::Int(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Str(value) => value,
            Self::Seq(_) | Self::Map(_) => String::new(),
        }
    }
}

impl<'de> Deserialize<'de> for Yaml {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(YamlVisitor)
    }
}

struct YamlVisitor;

impl<'de> Visitor<'de> for YamlVisitor {
    type Value = Yaml;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a YAML value")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Yaml, E> {
        Ok(Yaml::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Yaml, E> {
        Ok(Yaml::Int(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Yaml, E> {
        Ok(i64::try_from(value).map_or(Yaml::Float(value as f64), Yaml::Int))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Yaml, E> {
        Ok(Yaml::Float(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Yaml, E> {
        Ok(Yaml::Str(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Yaml, E> {
        Ok(Yaml::Str(value))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Yaml, E> {
        Ok(Yaml::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Yaml, E> {
        Ok(Yaml::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Yaml, D::Error> {
        Yaml::deserialize(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Yaml, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Yaml::Seq(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Yaml, A::Error> {
        let mut entries: Vec<(String, Yaml)> = Vec::new();
        while let Some((key, value)) = map.next_entry::<Yaml, Yaml>()? {
            let key = key.key();
            // A repeated key replaces the earlier value in place, as a Ruby hash does.
            match entries.iter_mut().find(|(existing, _)| *existing == key) {
                Some(entry) => entry.1 = value,
                None => entries.push((key, value)),
            }
        }
        Ok(Yaml::Map(entries))
    }
}

/// Parses `text` as one YAML document. An empty document is `Null`.
pub(crate) fn parse(text: &str) -> Result<Yaml, String> {
    if text.trim().is_empty() {
        return Ok(Yaml::Null);
    }
    serde_saphyr::from_str::<Yaml>(text).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{parse, Yaml};

    #[test]
    fn mappings_keep_their_order() {
        let yaml = parse("b: 1\na: two\nc: [1.5, ~]\n").unwrap();
        assert_eq!(
            yaml,
            Yaml::Map(vec![
                ("b".into(), Yaml::Int(1)),
                ("a".into(), Yaml::Str("two".into())),
                ("c".into(), Yaml::Seq(vec![Yaml::Float(1.5), Yaml::Null])),
            ])
        );
    }

    #[test]
    fn hex_colours_read_as_ruby_reads_them() {
        let yaml = parse("a: 333333\nb: F5F5F5\nc: '000000'\n").unwrap();
        let Yaml::Map(entries) = yaml else { panic!() };
        assert_eq!(entries[0].1, Yaml::Int(333_333));
        assert_eq!(entries[1].1, Yaml::Str("F5F5F5".into()));
        assert_eq!(entries[2].1, Yaml::Str("000000".into()));
    }
}
