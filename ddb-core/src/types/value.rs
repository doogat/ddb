use std::collections::BTreeMap;
use std::fmt;

/// Domain-level value type, decoupled from serde_yaml::Value.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Value {
    String(String),
    Number(f64),
    Bool(bool),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

// ── Path navigation ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    KeyNotFound {
        path: String,
        segment: String,
    },
    IndexOutOfBounds {
        path: String,
        index: usize,
        length: usize,
    },
    TypeMismatch {
        path: String,
        expected: &'static str,
        actual: &'static str,
    },
    InvalidPath {
        path: String,
        reason: String,
    },
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::KeyNotFound { path, segment } => {
                write!(f, "key not found: \"{segment}\" in path \"{path}\"")
            }
            PathError::IndexOutOfBounds {
                path,
                index,
                length,
            } => {
                write!(
                    f,
                    "index {index} out of bounds (length {length}) in path \"{path}\""
                )
            }
            PathError::TypeMismatch {
                path,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "type mismatch at \"{path}\": expected {expected}, got {actual}"
                )
            }
            PathError::InvalidPath { path, reason } => {
                write!(f, "invalid path \"{path}\": {reason}")
            }
        }
    }
}

impl std::error::Error for PathError {}

fn parse_bracket_index(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    path: &str,
) -> std::result::Result<PathSegment, PathError> {
    let mut index_str = String::new();
    loop {
        match chars.next() {
            Some(']') => break,
            Some(d) if d.is_ascii_digit() => index_str.push(d),
            Some(other) => {
                return Err(PathError::InvalidPath {
                    path: path.to_string(),
                    reason: format!("unexpected '{other}' in index"),
                });
            }
            None => {
                return Err(PathError::InvalidPath {
                    path: path.to_string(),
                    reason: "unclosed bracket".to_string(),
                });
            }
        }
    }
    if index_str.is_empty() {
        return Err(PathError::InvalidPath {
            path: path.to_string(),
            reason: "empty index".to_string(),
        });
    }
    let idx: usize = index_str.parse().map_err(|_| PathError::InvalidPath {
        path: path.to_string(),
        reason: format!("invalid index: {index_str}"),
    })?;
    Ok(PathSegment::Index(idx))
}

fn handle_dot(
    segments: &mut Vec<PathSegment>,
    current_key: &mut String,
    path: &str,
) -> std::result::Result<(), PathError> {
    if current_key.is_empty() {
        if !matches!(segments.last(), Some(PathSegment::Index(_))) {
            return Err(PathError::InvalidPath {
                path: path.to_string(),
                reason: "empty segment".to_string(),
            });
        }
    } else {
        segments.push(PathSegment::Key(std::mem::take(current_key)));
    }
    Ok(())
}

fn finalize_segments(
    segments: Vec<PathSegment>,
    last_was_dot: bool,
    current_key: String,
    path: &str,
) -> std::result::Result<Vec<PathSegment>, PathError> {
    if last_was_dot {
        return Err(PathError::InvalidPath {
            path: path.to_string(),
            reason: "trailing dot".to_string(),
        });
    }
    let mut segments = segments;
    if !current_key.is_empty() {
        segments.push(PathSegment::Key(current_key));
    }
    if segments.is_empty() {
        return Err(PathError::InvalidPath {
            path: path.to_string(),
            reason: "no segments".to_string(),
        });
    }
    Ok(segments)
}

/// Parse a dot/bracket notation path into segments.
///
/// - `.` separates map keys
/// - `[N]` indexes into lists (0-based)
/// - `\.` is a literal dot within a key name
/// - Empty segments are rejected
pub fn parse_path(path: &str) -> std::result::Result<Vec<PathSegment>, PathError> {
    if path.is_empty() {
        return Err(PathError::InvalidPath {
            path: path.to_string(),
            reason: "empty path".to_string(),
        });
    }

    let mut segments = Vec::new();
    let mut current_key = String::new();
    let mut chars = path.chars().peekable();
    let mut last_was_dot = false;

    while let Some(ch) = chars.next() {
        last_was_dot = false;
        match ch {
            '\\' => match chars.next() {
                Some(escaped) => current_key.push(escaped),
                None => current_key.push('\\'),
            },
            '.' => {
                last_was_dot = true;
                handle_dot(&mut segments, &mut current_key, path)?;
            }
            '[' => {
                if !current_key.is_empty() {
                    segments.push(PathSegment::Key(std::mem::take(&mut current_key)));
                }
                segments.push(parse_bracket_index(&mut chars, path)?);
            }
            _ => current_key.push(ch),
        }
    }

    finalize_segments(segments, last_was_dot, current_key, path)
}

fn traverse_segments<'a>(
    current: &'a Value,
    segments: &[PathSegment],
    path: &str,
) -> std::result::Result<&'a Value, PathError> {
    let mut current = current;
    for seg in segments {
        match seg {
            PathSegment::Key(k) => match current {
                Value::Map(m) => {
                    current = m.get(k).ok_or_else(|| PathError::KeyNotFound {
                        path: path.to_string(),
                        segment: k.clone(),
                    })?;
                }
                other => {
                    return Err(PathError::TypeMismatch {
                        path: path.to_string(),
                        expected: "map",
                        actual: other.type_name(),
                    });
                }
            },
            PathSegment::Index(idx) => match current {
                Value::List(list) => {
                    let len = list.len();
                    current = list.get(*idx).ok_or_else(|| PathError::IndexOutOfBounds {
                        path: path.to_string(),
                        index: *idx,
                        length: len,
                    })?;
                }
                other => {
                    return Err(PathError::TypeMismatch {
                        path: path.to_string(),
                        expected: "list",
                        actual: other.type_name(),
                    });
                }
            },
        }
    }
    Ok(current)
}

/// Navigate a dot/bracket path starting from a `BTreeMap`, without wrapping in `Value::Map`.
/// Useful when you have `&BTreeMap<String, Value>` (e.g., `extra` fields) and want to avoid cloning.
pub fn get_path_in_map<'a>(
    map: &'a BTreeMap<String, Value>,
    path: &str,
) -> std::result::Result<&'a Value, PathError> {
    let segments = parse_path(path)?;
    if segments.is_empty() {
        return Err(PathError::InvalidPath {
            path: path.to_string(),
            reason: "no segments".to_string(),
        });
    }

    let first = &segments[0];
    let PathSegment::Key(key) = first else {
        return Err(PathError::TypeMismatch {
            path: path.to_string(),
            expected: "map",
            actual: "map (index on root)",
        });
    };
    let first_value = map.get(key).ok_or_else(|| PathError::KeyNotFound {
        path: path.to_string(),
        segment: key.clone(),
    })?;

    traverse_segments(first_value, &segments[1..], path)
}

fn assign_at_leaf(
    current: &mut Value,
    seg: &PathSegment,
    value: Value,
    path: &str,
) -> std::result::Result<(), PathError> {
    match seg {
        PathSegment::Key(key) => match current {
            Value::Map(map) => {
                map.insert(key.clone(), value);
                Ok(())
            }
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "map",
                actual: other.type_name(),
            }),
        },
        PathSegment::Index(idx) => match current {
            Value::List(list) => {
                while list.len() <= *idx {
                    list.push(Value::String(String::new()));
                }
                list[*idx] = value;
                Ok(())
            }
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "list",
                actual: other.type_name(),
            }),
        },
    }
}

fn navigate_or_create<'a>(
    current: &'a mut Value,
    seg: &PathSegment,
    next_is_index: bool,
    path: &str,
) -> std::result::Result<&'a mut Value, PathError> {
    match seg {
        PathSegment::Key(key) => match current {
            Value::Map(map) => {
                Ok(map.entry(key.clone()).or_insert_with(|| {
                    if next_is_index {
                        Value::List(Vec::new())
                    } else {
                        Value::Map(BTreeMap::new())
                    }
                }))
            }
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "map",
                actual: other.type_name(),
            }),
        },
        PathSegment::Index(idx) => match current {
            Value::List(list) => {
                while list.len() <= *idx {
                    list.push(if next_is_index {
                        Value::List(Vec::new())
                    } else {
                        Value::Map(BTreeMap::new())
                    });
                }
                Ok(&mut list[*idx])
            }
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "list",
                actual: other.type_name(),
            }),
        },
    }
}

fn navigate_to_parent_mut<'a>(
    current: &'a mut Value,
    segments: &[PathSegment],
    path: &str,
) -> std::result::Result<&'a mut Value, PathError> {
    let mut current = current;
    for seg in segments {
        match seg {
            PathSegment::Key(key) => match current {
                Value::Map(map) => {
                    current = map.get_mut(key).ok_or_else(|| PathError::KeyNotFound {
                        path: path.to_string(),
                        segment: key.clone(),
                    })?;
                }
                other => {
                    return Err(PathError::TypeMismatch {
                        path: path.to_string(),
                        expected: "map",
                        actual: other.type_name(),
                    });
                }
            },
            PathSegment::Index(idx) => match current {
                Value::List(list) => {
                    let len = list.len();
                    current =
                        list.get_mut(*idx)
                            .ok_or_else(|| PathError::IndexOutOfBounds {
                                path: path.to_string(),
                                index: *idx,
                                length: len,
                            })?;
                }
                other => {
                    return Err(PathError::TypeMismatch {
                        path: path.to_string(),
                        expected: "list",
                        actual: other.type_name(),
                    });
                }
            },
        }
    }
    Ok(current)
}

fn remove_from_container(
    container: &mut Value,
    seg: &PathSegment,
    path: &str,
) -> std::result::Result<Value, PathError> {
    match seg {
        PathSegment::Key(key) => match container {
            Value::Map(map) => map.remove(key).ok_or_else(|| PathError::KeyNotFound {
                path: path.to_string(),
                segment: key.clone(),
            }),
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "map",
                actual: other.type_name(),
            }),
        },
        PathSegment::Index(idx) => match container {
            Value::List(list) => {
                if *idx >= list.len() {
                    Err(PathError::IndexOutOfBounds {
                        path: path.to_string(),
                        index: *idx,
                        length: list.len(),
                    })
                } else {
                    Ok(list.remove(*idx))
                }
            }
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "list",
                actual: other.type_name(),
            }),
        },
    }
}

impl Value {
    fn type_name(&self) -> &'static str {
        match self {
            Value::String(_) => "string",
            Value::Number(_) => "number",
            Value::Bool(_) => "bool",
            Value::List(_) => "list",
            Value::Map(_) => "map",
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_sequence(&self) -> Option<&[Value]> {
        match self {
            Value::List(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_mapping(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    pub fn is_sequence(&self) -> bool {
        matches!(self, Value::List(_))
    }

    pub fn is_mapping(&self) -> bool {
        matches!(self, Value::Map(_))
    }

    /// Navigate a nested `Value` tree using dot/bracket path notation.
    pub fn get_path(&self, path: &str) -> std::result::Result<&Value, PathError> {
        let segments = parse_path(path)?;
        traverse_segments(self, &segments, path)
    }

    /// Set a value at a dot/bracket path, creating intermediate containers as needed.
    pub fn set_path(&mut self, path: &str, value: Value) -> std::result::Result<(), PathError> {
        let segments = parse_path(path)?;
        let mut current = self;

        for (i, seg) in segments.iter().enumerate() {
            if i == segments.len() - 1 {
                return assign_at_leaf(current, seg, value, path);
            }
            let next_is_index = matches!(segments.get(i + 1), Some(PathSegment::Index(_)));
            current = navigate_or_create(current, seg, next_is_index, path)?;
        }

        Ok(())
    }

    /// Remove a value at a dot/bracket path, returning the removed value.
    pub fn remove_path(&mut self, path: &str) -> std::result::Result<Value, PathError> {
        let segments = parse_path(path)?;

        if segments.len() == 1 {
            return remove_from_container(self, &segments[0], path);
        }

        let last = &segments[segments.len() - 1];
        let parent = navigate_to_parent_mut(self, &segments[..segments.len() - 1], path)?;
        remove_from_container(parent, last, path)
    }

    // ── Type-safe path accessors ────────────────────────────────────

    pub fn str_at(&self, path: &str) -> std::result::Result<&str, PathError> {
        match self.get_path(path)? {
            Value::String(s) => Ok(s),
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "string",
                actual: other.type_name(),
            }),
        }
    }

    pub fn f64_at(&self, path: &str) -> std::result::Result<f64, PathError> {
        match self.get_path(path)? {
            Value::Number(n) => Ok(*n),
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "number",
                actual: other.type_name(),
            }),
        }
    }

    pub fn bool_at(&self, path: &str) -> std::result::Result<bool, PathError> {
        match self.get_path(path)? {
            Value::Bool(b) => Ok(*b),
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "bool",
                actual: other.type_name(),
            }),
        }
    }

    pub fn list_at(&self, path: &str) -> std::result::Result<&[Value], PathError> {
        match self.get_path(path)? {
            Value::List(v) => Ok(v),
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "list",
                actual: other.type_name(),
            }),
        }
    }

    pub fn map_at(&self, path: &str) -> std::result::Result<&BTreeMap<String, Value>, PathError> {
        match self.get_path(path)? {
            Value::Map(m) => Ok(m),
            other => Err(PathError::TypeMismatch {
                path: path.to_string(),
                expected: "map",
                actual: other.type_name(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Path navigation tests ───────────────────────────────────────

    fn nested_map() -> Value {
        let mut inner = BTreeMap::new();
        inner.insert("name".to_string(), Value::String("Alice".to_string()));
        inner.insert("age".to_string(), Value::Number(30.0));

        let mut deep = BTreeMap::new();
        deep.insert("city".to_string(), Value::String("NYC".to_string()));
        inner.insert("address".to_string(), Value::Map(deep));

        let mut root = BTreeMap::new();
        root.insert("author".to_string(), Value::Map(inner));
        root.insert(
            "tags".to_string(),
            Value::List(vec![
                Value::String("rust".to_string()),
                Value::String("doogat".to_string()),
            ]),
        );
        Value::Map(root)
    }

    #[test]
    fn path_parse_simple() {
        let segs = parse_path("a.b").unwrap();
        assert_eq!(
            segs,
            vec![PathSegment::Key("a".into()), PathSegment::Key("b".into())]
        );
    }

    #[test]
    fn path_parse_index() {
        let segs = parse_path("a[0]").unwrap();
        assert_eq!(
            segs,
            vec![PathSegment::Key("a".into()), PathSegment::Index(0)]
        );
    }

    #[test]
    fn path_parse_complex() {
        let segs = parse_path("a[0].b.c[2]").unwrap();
        assert_eq!(
            segs,
            vec![
                PathSegment::Key("a".into()),
                PathSegment::Index(0),
                PathSegment::Key("b".into()),
                PathSegment::Key("c".into()),
                PathSegment::Index(2),
            ]
        );
    }

    #[test]
    fn path_parse_escaped_dot() {
        let segs = parse_path(r"a\.b").unwrap();
        assert_eq!(segs, vec![PathSegment::Key("a.b".into())]);
    }

    #[test]
    fn path_parse_empty_rejected() {
        assert!(parse_path("").is_err());
        assert!(parse_path("a..b").is_err());
    }

    #[test]
    fn path_parse_trailing_dot_rejected() {
        assert!(parse_path("a.").is_err());
        assert!(parse_path("a.b.").is_err());
    }

    #[test]
    fn get_path_nested_map() {
        let v = nested_map();
        assert_eq!(
            v.get_path("author.name").unwrap(),
            &Value::String("Alice".into())
        );
        assert_eq!(
            v.get_path("author.address.city").unwrap(),
            &Value::String("NYC".into())
        );
    }

    #[test]
    fn get_path_list_index() {
        let v = nested_map();
        assert_eq!(
            v.get_path("tags[0]").unwrap(),
            &Value::String("rust".into())
        );
        assert_eq!(
            v.get_path("tags[1]").unwrap(),
            &Value::String("doogat".into())
        );
    }

    #[test]
    fn get_path_missing_key() {
        let v = nested_map();
        let err = v.get_path("author.email").unwrap_err();
        match err {
            PathError::KeyNotFound { segment, .. } => assert_eq!(segment, "email"),
            other => panic!("expected KeyNotFound, got {other}"),
        }
    }

    #[test]
    fn get_path_out_of_bounds() {
        let v = nested_map();
        let err = v.get_path("tags[5]").unwrap_err();
        match err {
            PathError::IndexOutOfBounds { index, length, .. } => {
                assert_eq!(index, 5);
                assert_eq!(length, 2);
            }
            other => panic!("expected IndexOutOfBounds, got {other}"),
        }
    }

    #[test]
    fn get_path_type_mismatch() {
        let v = nested_map();
        let err = v.get_path("author.name.foo").unwrap_err();
        match err {
            PathError::TypeMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, "map");
                assert_eq!(actual, "string");
            }
            other => panic!("expected TypeMismatch, got {other}"),
        }
    }

    #[test]
    fn set_path_creates_intermediates() {
        let mut v = Value::Map(BTreeMap::new());
        v.set_path("a.b.c", Value::Number(42.0)).unwrap();
        assert_eq!(v.get_path("a.b.c").unwrap(), &Value::Number(42.0));
    }

    #[test]
    fn set_path_replaces_existing() {
        let mut v = nested_map();
        v.set_path("author.name", Value::String("Bob".into()))
            .unwrap();
        assert_eq!(
            v.get_path("author.name").unwrap(),
            &Value::String("Bob".into())
        );
    }

    #[test]
    fn remove_path_returns_value() {
        let mut v = nested_map();
        let removed = v.remove_path("author.age").unwrap();
        assert_eq!(removed, Value::Number(30.0));
        assert!(v.get_path("author.age").is_err());
    }

    #[test]
    fn convenience_str_at() {
        let v = nested_map();
        assert_eq!(v.str_at("author.name").unwrap(), "Alice");
        let err = v.str_at("author.age").unwrap_err();
        match err {
            PathError::TypeMismatch { expected, .. } => assert_eq!(expected, "string"),
            other => panic!("expected TypeMismatch, got {other}"),
        }
    }

    #[test]
    fn convenience_f64_at() {
        let v = nested_map();
        assert_eq!(v.f64_at("author.age").unwrap(), 30.0);
        assert!(matches!(
            v.f64_at("author.name").unwrap_err(),
            PathError::TypeMismatch {
                expected: "number",
                ..
            }
        ));
    }

    #[test]
    fn convenience_bool_at() {
        let mut v = Value::Map(BTreeMap::new());
        v.set_path("flag", Value::Bool(true)).unwrap();
        assert!(v.bool_at("flag").unwrap());
        v.set_path("name", Value::String("x".into())).unwrap();
        assert!(matches!(
            v.bool_at("name").unwrap_err(),
            PathError::TypeMismatch {
                expected: "bool",
                ..
            }
        ));
    }

    #[test]
    fn convenience_list_at() {
        let v = nested_map();
        let tags = v.list_at("tags").unwrap();
        assert_eq!(tags.len(), 2);
        assert!(matches!(
            v.list_at("author.name").unwrap_err(),
            PathError::TypeMismatch {
                expected: "list",
                ..
            }
        ));
    }

    #[test]
    fn convenience_map_at() {
        let v = nested_map();
        let author = v.map_at("author").unwrap();
        assert!(author.contains_key("name"));
        assert!(matches!(
            v.map_at("author.name").unwrap_err(),
            PathError::TypeMismatch {
                expected: "map",
                ..
            }
        ));
    }

    #[test]
    fn round_trip() {
        let mut v = Value::Map(BTreeMap::new());
        let val = Value::List(vec![Value::Number(1.0), Value::Number(2.0)]);
        v.set_path("data.items", val.clone()).unwrap();
        assert_eq!(v.get_path("data.items").unwrap(), &val);
        assert_eq!(v.f64_at("data.items[0]").unwrap(), 1.0);
    }
}
