//! A JSON value that keeps its object keys in the order they were read, so
//! that `hippo mcp install --write` changes a client's file only where it
//! adds hippo. `serde_json::Value` sorts keys in this build.

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Json>),
    /// Keys in file order; a repeated key keeps its first place and its
    /// last value, as JSON parsers usually read it.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// `text` as JSON, refusing comments and trailing commas (a JSONC file).
    pub fn parse(text: &str) -> serde_json::Result<Json> {
        serde_json::from_str(text)
    }

    /// The value under `key` of an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The value under `key` of an object, inserted at the end as `default`
    /// when missing; `None` when `self` is not an object.
    pub fn entry(&mut self, key: &str, default: Json) -> Option<&mut Json> {
        let Json::Object(fields) = self else {
            return None;
        };
        let at = match fields.iter().position(|(k, _)| k == key) {
            Some(i) => i,
            None => {
                fields.push((key.to_owned(), default));
                fields.len() - 1
            }
        };
        Some(&mut fields[at].1)
    }

    /// Sets `key` of an object to `value`, in place when present, at the end
    /// otherwise. Returns false when `self` is not an object.
    pub fn set(&mut self, key: &str, value: Json) -> bool {
        match self.entry(key, Json::Null) {
            Some(slot) => {
                *slot = value;
                true
            }
            None => false,
        }
    }

    /// An object of the given fields.
    pub fn object(fields: Vec<(&str, Json)>) -> Json {
        Json::Object(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }

    pub fn string(s: impl Into<String>) -> Json {
        Json::String(s.into())
    }

    /// Two-space indented, with a final line feed.
    pub fn pretty(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Json::Null => s.serialize_unit(),
            Json::Bool(b) => s.serialize_bool(*b),
            Json::Number(n) => n.serialize(s),
            Json::String(t) => s.serialize_str(t),
            Json::Array(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Json::Object(fields) => {
                let mut map = s.serialize_map(Some(fields.len()))?;
                for (k, v) in fields {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Json, D::Error> {
        d.deserialize_any(JsonVisitor)
    }
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_unit<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_none<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_bool<E>(self, b: bool) -> Result<Json, E> {
        Ok(Json::Bool(b))
    }

    fn visit_i64<E>(self, n: i64) -> Result<Json, E> {
        Ok(Json::Number(n.into()))
    }

    fn visit_u64<E>(self, n: u64) -> Result<Json, E> {
        Ok(Json::Number(n.into()))
    }

    fn visit_f64<E: de::Error>(self, n: f64) -> Result<Json, E> {
        serde_json::Number::from_f64(n)
            .map(Json::Number)
            .ok_or_else(|| E::custom("a number JSON cannot hold"))
    }

    fn visit_str<E>(self, s: &str) -> Result<Json, E> {
        Ok(Json::String(s.to_owned()))
    }

    fn visit_string<E>(self, s: String) -> Result<Json, E> {
        Ok(Json::String(s))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Json::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
        let mut out = Json::Object(Vec::new());
        while let Some((k, v)) = map.next_entry::<String, Json>()? {
            out.set(&k, v);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::Json;

    #[test]
    fn a_file_round_trips_in_its_own_key_order() {
        let text = "{\n  \"zeta\": 1,\n  \"alpha\": {\n    \"b\": [\n      true,\n      null,\n      2.5\n    ],\n    \"a\": \"x\"\n  }\n}\n";
        let json = Json::parse(text).unwrap();
        assert_eq!(json.pretty(), text);
    }

    #[test]
    fn comments_are_refused() {
        assert!(Json::parse("{ // a comment\n \"a\": 1 }").is_err());
        assert!(Json::parse("{ \"a\": 1, }").is_err());
    }

    #[test]
    fn set_replaces_in_place_and_appends_new_keys() {
        let mut json = Json::parse("{\"b\": 1, \"a\": 2}").unwrap();
        json.set("b", Json::Bool(false));
        json.set("c", Json::Null);
        assert_eq!(
            serde_json::to_string(&json).unwrap(),
            "{\"b\":false,\"a\":2,\"c\":null}"
        );
    }
}
