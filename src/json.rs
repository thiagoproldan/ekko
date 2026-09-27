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

/// Asserts that a `T` read from `fixture` and written back keeps what a
/// later version may add anywhere in it (task 829). A field no version
/// knows is put into every object at every depth, named by where it sits,
/// and each must come back where it was, and once: two maps that both take
/// the fields nobody knows write each of them twice, and a read keeps only
/// the last. `fixture` is as this version writes it, and holds every object
/// a `T` can hold, since an object it leaves out goes unchecked.
#[cfg(test)]
pub fn assert_keeps_what_it_does_not_know<T: Serialize + serde::de::DeserializeOwned>(fixture: serde_json::Value) {
    use serde_json::Value;
    fn plant(value: &mut Value, at: &str, planted: &mut usize) {
        match value {
            Value::Object(fields) => {
                for (name, inner) in fields.iter_mut() {
                    plant(inner, &format!("{at}.{name}"), planted);
                }
                fields.insert(format!("later {at}"), Value::String(at.to_string()));
                *planted += 1;
            }
            Value::Array(items) => {
                for (i, inner) in items.iter_mut().enumerate() {
                    plant(inner, &format!("{at}[{i}]"), planted);
                }
            }
            _ => {}
        }
    }

    let read: T = serde_json::from_value(fixture.clone()).expect("the fixture reads");
    assert_eq!(serde_json::to_value(&read).unwrap(), fixture, "the fixture is as this version writes it");

    let mut later = fixture;
    let mut planted = 0;
    plant(&mut later, "$", &mut planted);
    let read: T = serde_json::from_value(later.clone()).expect("fields this version does not know do not stop a read");
    let written = serde_json::to_string(&read).unwrap();
    let back: Value = serde_json::from_str(&written).unwrap();
    assert_eq!(back, later, "every one of the {planted} planted fields comes back where it was");
    assert_eq!(written.len(), serde_json::to_string(&back).unwrap().len(), "a field is written twice: {written}");
}
