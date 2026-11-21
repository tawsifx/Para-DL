//! The single JSON serializer behind `emit_jsonl`.
//!
//! `serde_json` writes compact JSON (`{"a":1,"b":2}`), but the Python oracle
//! uses `json.dumps` defaults (`{"a": 1, "b": 2}`) with insertion-ordered keys
//! and `ensure_ascii=True`. This module implements `serde::Serializer` directly
//! so that **one** code path produces byte-identical JSONL for every event,
//! including typed events defined in other crates (e.g.
//! `paradl_watchdog::WatchdogActionEvent`).
//!
//! Redaction is applied while serializing: string values are passed through
//! [`Redactor::redact_text`] and sensitive mapping keys have their value
//! replaced wholesale with `***REDACTED***`. This is the Rust equivalent of
//! `UI.emit_event()` calling `_redact_obj` before `json.dumps`.

use std::fmt::Write as _;

use serde::Serialize;
use serde::Serializer;
use serde::ser::{
    Error as SerError, Impossible, SerializeMap, SerializeSeq, SerializeStruct, SerializeTuple,
    SerializeTupleStruct,
};

use crate::redaction::{REDACTED, Redactor};

/// The serializer is infallible; this uninhabited error type only satisfies serde's API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Never {}

impl std::fmt::Display for Never {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {}
    }
}

impl std::error::Error for Never {}

impl SerError for Never {
    fn custom<T: std::fmt::Display>(_message: T) -> Self {
        unreachable!("the JSONL serializer is infallible")
    }
}

/// Helper for the two serde hooks that cannot occur in an event payload.
fn unsupported<T>() -> Result<T, Never> {
    unreachable!("variant serializers are not used by paraDL events")
}

/// Serialize any `Serialize` value into a Python-`json.dumps`-style JSON line.
pub(crate) struct PySerializer {
    out: String,
    redactor: Redactor,
}

impl PySerializer {
    /// Create a serializer with the given redaction configuration.
    pub(crate) fn new(redactor: Redactor) -> Self {
        Self {
            out: String::new(),
            redactor,
        }
    }

    /// Consume the serializer and return the accumulated text.
    pub(crate) fn finish(self) -> String {
        self.out
    }

    fn push_string(&mut self, value: &str) {
        self.out.push_str(&py_quote(&self.redactor.redact_text(value)));
    }

    fn push_redacted(&mut self) {
        self.out.push('"');
        self.out.push_str(REDACTED);
        self.out.push('"');
    }
}

/// Python `json.dumps` string quoting with `ensure_ascii=True`.
fn py_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c if (c as u32) <= 0x7f => out.push(c),
            c => {
                let code = c as u32;
                if code <= 0xffff {
                    let _ = write!(out, "\\u{code:04x}");
                } else {
                    let shifted = code - 0x10000;
                    let high = 0xd800 + (shifted >> 10);
                    let low = 0xdc00 + (shifted & 0x3ff);
                    let _ = write!(out, "\\u{high:04x}\\u{low:04x}");
                }
            }
        }
    }
    out.push('"');
    out
}

/// Python-compatible float rendering (`50.0`, not `50`; lowest-roundtrip digits
/// otherwise). Non-finite values become `null` so output stays valid JSON.
fn py_float(value: f64) -> String {
    if !value.is_finite() {
        return "null".to_string();
    }
    if value.fract() == 0.0 && value.abs() < 1.0e16 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

impl<'a> Serializer for &'a mut PySerializer {
    type Ok = ();
    type Error = Never;
    type SerializeSeq = Seq<'a>;
    type SerializeTuple = Seq<'a>;
    type SerializeTupleStruct = Seq<'a>;
    type SerializeTupleVariant = Impossible<(), Never>;
    type SerializeMap = Map<'a>;
    type SerializeStruct = Struct<'a>;
    type SerializeStructVariant = Impossible<(), Never>;

    fn serialize_bool(self, value: bool) -> Result<(), Never> {
        self.out.push_str(if value { "true" } else { "false" });
        Ok(())
    }

    fn serialize_i8(self, value: i8) -> Result<(), Never> {
        let _ = write!(self.out, "{value}");
        Ok(())
    }

    fn serialize_i16(self, value: i16) -> Result<(), Never> {
        let _ = write!(self.out, "{value}");
        Ok(())
    }

    fn serialize_i32(self, value: i32) -> Result<(), Never> {
        let _ = write!(self.out, "{value}");
        Ok(())
    }

    fn serialize_i64(self, value: i64) -> Result<(), Never> {
        let _ = write!(self.out, "{value}");
        Ok(())
    }

    fn serialize_u8(self, value: u8) -> Result<(), Never> {
        let _ = write!(self.out, "{value}");
        Ok(())
    }

    fn serialize_u16(self, value: u16) -> Result<(), Never> {
        let _ = write!(self.out, "{value}");
        Ok(())
    }

    fn serialize_u32(self, value: u32) -> Result<(), Never> {
        let _ = write!(self.out, "{value}");
        Ok(())
    }

    fn serialize_u64(self, value: u64) -> Result<(), Never> {
        let _ = write!(self.out, "{value}");
        Ok(())
    }

    fn serialize_f32(self, value: f32) -> Result<(), Never> {
        self.out.push_str(&py_float(value as f64));
        Ok(())
    }

    fn serialize_f64(self, value: f64) -> Result<(), Never> {
        self.out.push_str(&py_float(value));
        Ok(())
    }

    fn serialize_char(self, value: char) -> Result<(), Never> {
        self.push_string(&value.to_string());
        Ok(())
    }

    fn serialize_str(self, value: &str) -> Result<(), Never> {
        self.push_string(value);
        Ok(())
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<(), Never> {
        self.push_string(&String::from_utf8_lossy(value));
        Ok(())
    }

    fn serialize_none(self) -> Result<(), Never> {
        self.out.push_str("null");
        Ok(())
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<(), Never> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<(), Never> {
        self.out.push_str("null");
        Ok(())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), Never> {
        self.out.push_str("null");
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<(), Never> {
        self.out.push_str(&py_quote(variant));
        Ok(())
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), Never> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        value: &T,
    ) -> Result<(), Never> {
        value.serialize(self)
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Never> {
        self.out.push('[');
        Ok(Seq {
            ser: self,
            first: true,
        })
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Never> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Never> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Never> {
        unsupported()
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Never> {
        self.out.push('{');
        Ok(Map {
            ser: self,
            first: true,
            key: String::new(),
        })
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Never> {
        self.out.push('{');
        Ok(Struct {
            ser: self,
            first: true,
        })
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Never> {
        unsupported()
    }
}

/// Sequence / tuple writer.
pub(crate) struct Seq<'a> {
    ser: &'a mut PySerializer,
    first: bool,
}

impl SerializeSeq for Seq<'_> {
    type Ok = ();
    type Error = Never;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Never> {
        if !self.first {
            self.ser.out.push_str(", ");
        }
        self.first = false;
        value.serialize(&mut *self.ser)
    }

    fn end(self) -> Result<(), Never> {
        self.ser.out.push(']');
        Ok(())
    }
}

impl SerializeTuple for Seq<'_> {
    type Ok = ();
    type Error = Never;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Never> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<(), Never> {
        SerializeSeq::end(self)
    }
}

impl SerializeTupleStruct for Seq<'_> {
    type Ok = ();
    type Error = Never;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Never> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<(), Never> {
        SerializeSeq::end(self)
    }
}

/// Map writer (also used by `#[serde(flatten)]` payloads).
pub(crate) struct Map<'a> {
    ser: &'a mut PySerializer,
    first: bool,
    key: String,
}

impl SerializeMap for Map<'_> {
    type Ok = ();
    type Error = Never;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Never> {
        let mut probe = PySerializer::new(Redactor::new());
        key.serialize(&mut probe)?;
        self.key = probe.finish();
        Ok(())
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Never> {
        if !self.first {
            self.ser.out.push_str(", ");
        }
        self.first = false;
        self.ser.out.push_str(&self.key);
        self.ser.out.push_str(": ");
        let plain_key = self
            .key
            .strip_prefix('"')
            .and_then(|text| text.strip_suffix('"'))
            .unwrap_or(&self.key);
        if Redactor::is_sensitive_key(plain_key) {
            self.ser.push_redacted();
        } else {
            value.serialize(&mut *self.ser)?;
        }
        Ok(())
    }

    fn serialize_entry<K: ?Sized + Serialize, V: ?Sized + Serialize>(
        &mut self,
        key: &K,
        value: &V,
    ) -> Result<(), Never> {
        self.serialize_key(key)?;
        self.serialize_value(value)
    }

    fn end(self) -> Result<(), Never> {
        self.ser.out.push('}');
        Ok(())
    }
}

/// Struct writer. Field order is declaration order, matching Python dicts.
pub(crate) struct Struct<'a> {
    ser: &'a mut PySerializer,
    first: bool,
}

impl SerializeStruct for Struct<'_> {
    type Ok = ();
    type Error = Never;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Never> {
        if !self.first {
            self.ser.out.push_str(", ");
        }
        self.first = false;
        self.ser.out.push_str(&py_quote(key));
        self.ser.out.push_str(": ");
        if Redactor::is_sensitive_key(key) {
            self.ser.push_redacted();
        } else {
            value.serialize(&mut *self.ser)?;
        }
        Ok(())
    }

    fn skip_field(&mut self, _key: &'static str) -> Result<(), Never> {
        Ok(())
    }

    fn end(self) -> Result<(), Never> {
        self.ser.out.push('}');
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_python_dumps_defaults() {
        #[derive(Serialize)]
        struct Sample {
            event: &'static str,
            n: i32,
            f: f64,
            list: Vec<i32>,
            none: Option<i32>,
            text: &'static str,
        }
        let value = Sample {
            event: "progress",
            n: 16,
            f: 50.0,
            list: vec![1, 2],
            none: None,
            text: "caf\u{e9}",
        };
        let mut ser = PySerializer::new(Redactor::new());
        value.serialize(&mut ser).unwrap();
        assert_eq!(
            ser.finish(),
            "{\"event\": \"progress\", \"n\": 16, \"f\": 50.0, \"list\": [1, 2], \"none\": null, \"text\": \"caf\\u00e9\"}"
        );
    }
}
