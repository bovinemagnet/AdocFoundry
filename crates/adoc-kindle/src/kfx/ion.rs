//! Amazon Ion 1.0 binary values, as KFX stores every fragment, written from the public Ion
//! specification: a type descriptor byte (type in the high nibble, length in the low, 14 for a
//! `VarUInt` length after it), then the representation.

/// The Ion 1.0 binary version marker that starts every stream.
pub(crate) const VERSION_MARKER: [u8; 4] = [0xE0, 0x01, 0x00, 0xEA];

/// A value, its symbols and field names given as symbol ids.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Bool(bool),
    Int(i64),
    /// `coefficient × 10^exponent`.
    Decimal {
        coefficient: i64,
        exponent: i32,
    },
    Symbol(u32),
    String(String),
    List(Vec<Value>),
    Struct(Vec<(u32, Value)>),
    Annotated(Vec<u32>, Box<Value>),
}

/// `values` as a stream, the version marker first.
pub(crate) fn stream(values: &[Value]) -> Vec<u8> {
    let mut out = VERSION_MARKER.to_vec();
    for value in values {
        write(&mut out, value);
    }
    out
}

/// A `VarUInt`: seven bits a byte, most significant first, the last byte's high bit set.
pub(crate) fn var_uint(out: &mut Vec<u8>, value: u64) {
    let mut groups = vec![(value & 0x7F) as u8 | 0x80];
    let mut rest = value >> 7;
    while rest != 0 {
        groups.push((rest & 0x7F) as u8);
        rest >>= 7;
    }
    groups.reverse();
    out.extend_from_slice(&groups);
}

/// A `VarInt`: as a `VarUInt`, with the first byte's second bit the sign.
fn var_int(out: &mut Vec<u8>, value: i64) {
    let magnitude = value.unsigned_abs();
    let mut groups = vec![(magnitude & 0x7F) as u8];
    let mut rest = magnitude >> 7;
    while rest != 0 {
        groups.push((rest & 0x7F) as u8);
        rest >>= 7;
    }
    // The first byte holds six bits besides the sign.
    if groups.last().is_some_and(|first| first & 0x40 != 0) {
        groups.push(0);
    }
    groups.reverse();
    if value < 0 {
        groups[0] |= 0x40;
    }
    let last = groups.len() - 1;
    groups[last] |= 0x80;
    out.extend_from_slice(&groups);
}

/// `value`'s magnitude in as few big-endian bytes as hold it; none for zero.
fn uint_bytes(value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let first = bytes.iter().position(|byte| *byte != 0).unwrap_or(8);
    bytes[first..].to_vec()
}

fn descriptor(out: &mut Vec<u8>, kind: u8, length: usize) {
    if length < 14 {
        out.push(kind << 4 | length as u8);
    } else {
        out.push(kind << 4 | 14);
        var_uint(out, length as u64);
    }
}

pub(crate) fn write(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Bool(value) => out.push(0x10 | u8::from(*value)),
        Value::Int(value) => {
            let magnitude = uint_bytes(value.unsigned_abs());
            descriptor(out, if *value < 0 { 3 } else { 2 }, magnitude.len());
            out.extend_from_slice(&magnitude);
        }
        Value::Decimal {
            coefficient,
            exponent,
        } => {
            let mut body = Vec::new();
            var_int(&mut body, i64::from(*exponent));
            // The coefficient as an Ion `Int`: sign and magnitude, the sign in the first bit.
            if *coefficient != 0 {
                let mut magnitude = uint_bytes(coefficient.unsigned_abs());
                if magnitude[0] & 0x80 != 0 {
                    magnitude.insert(0, 0);
                }
                if *coefficient < 0 {
                    magnitude[0] |= 0x80;
                }
                body.extend_from_slice(&magnitude);
            }
            descriptor(out, 5, body.len());
            out.extend_from_slice(&body);
        }
        Value::Symbol(id) => {
            let bytes = uint_bytes(u64::from(*id));
            descriptor(out, 7, bytes.len());
            out.extend_from_slice(&bytes);
        }
        Value::String(text) => {
            descriptor(out, 8, text.len());
            out.extend_from_slice(text.as_bytes());
        }
        Value::List(items) => {
            let mut body = Vec::new();
            for item in items {
                write(&mut body, item);
            }
            descriptor(out, 11, body.len());
            out.extend_from_slice(&body);
        }
        Value::Struct(fields) => {
            let mut body = Vec::new();
            for (name, field) in fields {
                var_uint(&mut body, u64::from(*name));
                write(&mut body, field);
            }
            // A length of one would mean a sorted struct; none is written here, and a struct
            // that short cannot hold a field anyway.
            descriptor(out, 13, body.len());
            out.extend_from_slice(&body);
        }
        Value::Annotated(annotations, inner) => {
            let mut names = Vec::new();
            for annotation in annotations {
                var_uint(&mut names, u64::from(*annotation));
            }
            let mut body = Vec::new();
            var_uint(&mut body, names.len() as u64);
            body.extend_from_slice(&names);
            write(&mut body, inner);
            descriptor(out, 14, body.len());
            out.extend_from_slice(&body);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(value: &Value) -> Vec<u8> {
        let mut out = Vec::new();
        write(&mut out, value);
        out
    }

    #[test]
    fn values_have_their_specification_encodings() {
        assert_eq!(bytes(&Value::Int(0)), [0x20]);
        assert_eq!(bytes(&Value::Int(1)), [0x21, 0x01]);
        assert_eq!(bytes(&Value::Int(-1)), [0x31, 0x01]);
        assert_eq!(bytes(&Value::Int(300)), [0x22, 0x01, 0x2C]);
        assert_eq!(bytes(&Value::Bool(true)), [0x11]);
        assert_eq!(bytes(&Value::Symbol(4)), [0x71, 0x04]);
        assert_eq!(bytes(&Value::String("ab".into())), [0x82, b'a', b'b']);
        // 1 and 1.2, as KFX writes line heights and sizes.
        assert_eq!(
            bytes(&Value::Decimal {
                coefficient: 1,
                exponent: 0
            }),
            [0x52, 0x80, 0x01]
        );
        assert_eq!(
            bytes(&Value::Decimal {
                coefficient: 12,
                exponent: -1
            }),
            [0x52, 0xC1, 0x0C]
        );
        assert_eq!(
            bytes(&Value::Struct(vec![
                (410, Value::Int(0)),
                (411, Value::Int(0))
            ])),
            [0xD6, 0x03, 0x9A, 0x20, 0x03, 0x9B, 0x20]
        );
        assert_eq!(
            bytes(&Value::Annotated(vec![3], Box::new(Value::Int(0)))),
            [0xE3, 0x81, 0x83, 0x20]
        );
        let long = "x".repeat(20);
        assert_eq!(&bytes(&Value::String(long))[..2], [0x8E, 0x94]);
    }
}
