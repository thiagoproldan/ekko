//! 4-space-indented JSON, matching the JS version's
//! `JSON.stringify(data, null, 4)` exactly -- `serde_json::to_string_pretty`
//! defaults to 2 spaces and isn't configurable through that function
//! directly, hence this small wrapper instead of calling it at each site.

use serde::Serialize;

pub fn to_pretty_string<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let mut buffer = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
    value.serialize(&mut serializer)?;
    Ok(String::from_utf8(buffer).expect("serde_json only ever writes valid UTF-8"))
}
