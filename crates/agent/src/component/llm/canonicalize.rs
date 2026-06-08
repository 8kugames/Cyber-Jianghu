//! JSON schema 规范化, 让 DeepSeek tools 字段字节级稳定

use serde_json::Value;

pub fn canonicalize_json_schema(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                canonicalize_json_schema(v);
            }
            let entries: Vec<_> = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let mut sorted = entries;
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            map.clear();
            for (k, v) in sorted {
                map.insert(k, v);
            }
            if let Some(Value::Array(arr)) = map.get_mut("required") {
                let mut sorted_arr: Vec<Value> = std::mem::take(arr);
                sorted_arr.sort_by(|a, b| {
                    let a_s = a.as_str().unwrap_or("");
                    let b_s = b.as_str().unwrap_or("");
                    a_s.cmp(b_s)
                });
                *arr = sorted_arr;
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                canonicalize_json_schema(v);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonicalize_sorts_object_keys() {
        let mut v = json!({"z": 1, "a": 2, "m": 3});
        canonicalize_json_schema(&mut v);
        let s = v.to_string();
        assert_eq!(s, r#"{"a":2,"m":3,"z":1}"#);
    }

    #[test]
    fn canonicalize_sorts_required_array() {
        let mut v = json!({"required": ["z", "a", "m"]});
        canonicalize_json_schema(&mut v);
        assert_eq!(v["required"], json!(["a", "m", "z"]));
    }

    #[test]
    fn canonicalize_recursive() {
        let mut v = json!({
            "z": {"y": 1, "x": 2},
            "a": [{"c": 1, "b": 2}]
        });
        canonicalize_json_schema(&mut v);
        assert_eq!(v.to_string(), r#"{"a":[{"b":2,"c":1}],"z":{"x":2,"y":1}}"#);
    }
}
