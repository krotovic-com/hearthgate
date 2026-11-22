//! A small JSON Patch (RFC 6902 `add`, `remove` and `replace`), for the
//! changes a suggestion makes to the desired state.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One change. `value` is unused for `remove`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PatchOp {
    pub op: Op,
    /// JSON Pointer, such as `/firewall/rules/0`.
    pub path: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Add,
    Remove,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatchError {
    #[error("{0}: no such place in the configuration")]
    Missing(String),
    #[error("{0}: not a valid path")]
    BadPath(String),
}

fn unescape(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

fn split(path: &str) -> Result<(Vec<String>, String), PatchError> {
    let rest = path
        .strip_prefix('/')
        .ok_or_else(|| PatchError::BadPath(path.to_owned()))?;
    let mut tokens: Vec<String> = rest.split('/').map(unescape).collect();
    let last = tokens.pop().unwrap_or_default();
    Ok((tokens, last))
}

/// Walks to the parent of the target. With `create`, missing objects on the
/// way are made, as models often add to a section that the state leaves
/// out while it is at its default.
fn parent<'a>(
    root: &'a mut Value,
    tokens: &[String],
    path: &str,
    create: bool,
) -> Result<&'a mut Value, PatchError> {
    let mut here = root;
    for t in tokens {
        here = match here {
            Value::Object(map) => {
                if create {
                    Some(
                        map.entry(t.clone())
                            .or_insert_with(|| Value::Object(serde_json::Map::new())),
                    )
                } else {
                    map.get_mut(t)
                }
            }
            Value::Array(items) => t.parse::<usize>().ok().and_then(|i| items.get_mut(i)),
            _ => None,
        }
        .ok_or_else(|| PatchError::Missing(path.to_owned()))?;
    }
    Ok(here)
}

/// Applies `ops` in order. On an error, `doc` may be partly changed, so
/// apply to a copy.
///
/// # Errors
/// When a path is malformed or points nowhere.
pub fn apply(doc: &mut Value, ops: &[PatchOp]) -> Result<(), PatchError> {
    for op in ops {
        if op.path.is_empty() {
            match op.op {
                Op::Add | Op::Replace => *doc = op.value.clone(),
                Op::Remove => return Err(PatchError::BadPath(String::new())),
            }
            continue;
        }
        let (tokens, last) = split(&op.path)?;
        let target = parent(doc, &tokens, &op.path, op.op == Op::Add)?;
        let missing = || PatchError::Missing(op.path.clone());
        match (target, op.op) {
            (Value::Object(map), Op::Add) => {
                map.insert(last, op.value.clone());
            }
            (Value::Object(map), Op::Replace) => {
                *map.get_mut(&last).ok_or_else(missing)? = op.value.clone();
            }
            (Value::Object(map), Op::Remove) => {
                map.remove(&last).ok_or_else(missing)?;
            }
            (Value::Array(items), kind) => {
                let index = if last == "-" && kind == Op::Add {
                    items.len()
                } else {
                    last.parse::<usize>().map_err(|_| missing())?
                };
                match kind {
                    Op::Add if index <= items.len() => items.insert(index, op.value.clone()),
                    Op::Replace if index < items.len() => items[index] = op.value.clone(),
                    Op::Remove if index < items.len() => {
                        items.remove(index);
                    }
                    _ => return Err(missing()),
                }
            }
            _ => return Err(missing()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn op(op: Op, path: &str, value: Value) -> PatchOp {
        PatchOp {
            op,
            path: path.into(),
            value,
        }
    }

    #[test]
    fn applies_add_remove_replace() {
        let mut doc = json!({"firewall": {"rules": [{"a": 1}, {"b": 2}]}, "x": 1});
        apply(
            &mut doc,
            &[
                op(Op::Add, "/firewall/rules/-", json!({"c": 3})),
                op(Op::Remove, "/firewall/rules/0", Value::Null),
                op(Op::Replace, "/x", json!(2)),
                op(Op::Add, "/y~1z", json!(true)),
            ],
        )
        .unwrap();
        assert_eq!(
            doc,
            json!({"firewall": {"rules": [{"b": 2}, {"c": 3}]}, "x": 2, "y/z": true})
        );
    }

    #[test]
    fn refuses_bad_paths() {
        let mut doc = json!({"a": [1]});
        assert!(apply(&mut doc, &[op(Op::Replace, "/b", json!(1))]).is_err());
        assert!(apply(&mut doc, &[op(Op::Remove, "/a/3", Value::Null)]).is_err());
        assert!(apply(&mut doc, &[op(Op::Add, "a", json!(1))]).is_err());
        assert!(apply(&mut doc, &[op(Op::Add, "/a/x/y", json!(1))]).is_err());
        apply(&mut doc, &[op(Op::Add, "/router/upnp", json!(true))]).unwrap();
        assert_eq!(doc["router"]["upnp"], true);
    }
}
