use serde::Serialize;

pub fn to_pretty_json<T>(value: &T) -> serde_json::Result<String>
where
    T: Serialize,
{
    serde_json::to_string_pretty(value)
}
