//! OpenAPI 3.0 schema conversion for the hand-written `/api/openapi.json`
//! generator (mirror of origin #1447: `openApiSchema.ts` +
//! `resolveEffectiveSchema` from `parameterConversion.ts`).
//!
//! MCP tool input schemas are JSON Schema; the generated documents declare
//! OpenAPI 3.0.3, where `type` must be a string and nullability is expressed
//! with `nullable`. This module rewrites schema fragments accordingly.

use serde_json::{json, Map, Value};

fn definition_name(r#ref: &str) -> Option<&str> {
    for prefix in ["#/$defs/", "#/definitions/"] {
        if let Some(name) = r#ref.strip_prefix(prefix) {
            return Some(name);
        }
    }
    None
}

fn is_null_schema(member: &Value) -> bool {
    member.get("type").and_then(|t| t.as_str()) == Some("null")
}

/// Collect the local definitions of a tool input schema, keyed by name.
pub fn collect_definitions(root_schema: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    for key in ["definitions", "$defs"] {
        if let Some(Value::Object(defs)) = root_schema.get(key) {
            for (k, v) in defs {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

/// Rewrite a schema for OpenAPI 3.0:
/// - `type: [..., 'null']` and `anyOf`/`oneOf` members of `{ type: 'null' }` become `nullable: true`
/// - other `type` lists become an `anyOf`
/// - `const` becomes a single-value `enum`
/// - local `$ref`s to `$defs`/`definitions` are inlined, since the root definitions are not part
///   of the generated document. A reference to a definition that is already being inlined
///   (a recursive schema) becomes an empty schema, which accepts any value.
pub fn to_openapi_schema(schema: &Value, definitions: &Map<String, Value>) -> Value {
    let mut resolving = Vec::new();
    walk(schema, definitions, &mut resolving)
}

fn walk(schema: &Value, definitions: &Map<String, Value>, resolving: &mut Vec<String>) -> Value {
    if let Value::Array(items) = schema {
        return Value::Array(
            items
                .iter()
                .map(|item| walk(item, definitions, resolving))
                .collect(),
        );
    }
    if !schema.is_object() {
        return schema.clone();
    }

    let obj = schema.as_object().unwrap();
    let r#ref = obj.get("$ref").cloned();
    let rest: Map<String, Value> = obj
        .iter()
        .filter(|(k, _)| k.as_str() != "$ref" && k.as_str() != "$defs" && k.as_str() != "definitions")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let node = convert_children(&Value::Object(rest), definitions, resolving);

    if let Some(Value::String(r#ref)) = r#ref {
        if let Some(name) = definition_name(&r#ref) {
            if let Some(target) = definitions.get(name) {
                let converted = if resolving.iter().any(|r| r == name) {
                    // Recursive reference: an empty schema accepts any value.
                    Value::Object(Map::new())
                } else {
                    resolving.push(name.to_string());
                    let out = walk(target, definitions, resolving);
                    resolving.pop();
                    out
                };
                return normalize_nullability(&merge_shallow(&converted, &node));
            }
        }
        // Not a reference this converter can resolve; keep it as written.
        let mut out = Map::new();
        out.insert("$ref".to_string(), Value::String(r#ref));
        if let Value::Object(m) = node {
            for (k, v) in m {
                out.insert(k, v);
            }
        }
        return Value::Object(out);
    }

    normalize_nullability(&node)
}

/// Shallow merge where `b`'s keys win (origin `{ ...target, ...node }`).
fn merge_shallow(a: &Value, b: &Value) -> Value {
    let mut out = match a {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    if let Value::Object(m) = b {
        for (k, v) in m {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

fn convert_children(node: &Value, definitions: &Map<String, Value>, resolving: &mut Vec<String>) -> Value {
    let obj = match node.as_object() {
        Some(o) => o,
        None => return node.clone(),
    };
    let mut converted = Map::new();
    for (key, value) in obj {
        if key == "properties" && value.is_object() {
            let props: Map<String, Value> = value
                .as_object()
                .unwrap()
                .iter()
                .map(|(name, prop)| (name.clone(), walk(prop, definitions, resolving)))
                .collect();
            converted.insert(key.clone(), Value::Object(props));
        } else if ["items", "additionalProperties", "not"].contains(&key.as_str()) {
            converted.insert(key.clone(), walk(value, definitions, resolving));
        } else if ["allOf", "anyOf", "oneOf"].contains(&key.as_str()) && value.is_array() {
            converted.insert(
                key.clone(),
                Value::Array(
                    value
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|item| walk(item, definitions, resolving))
                        .collect(),
                ),
            );
        } else {
            converted.insert(key.clone(), value.clone());
        }
    }
    Value::Object(converted)
}

fn normalize_nullability(node: &Value) -> Value {
    let mut result = match node {
        Value::Object(m) => m.clone(),
        _ => return node.clone(),
    };
    let mut nullable = false;

    if let Some(c) = result.get("const").cloned() {
        if result.get("enum").is_none() {
            result.insert("enum".to_string(), Value::Array(vec![c]));
            result.remove("const");
        }
    }

    if let Some(Value::Array(types)) = result.get("type").cloned().as_ref() {
        let types: Vec<&Value> = types.iter().filter(|t| t.as_str() != Some("null")).collect();
        let accepts_null = types.len() != result["type"].as_array().unwrap().len();
        if types.is_empty() {
            result.remove("type");
        } else if types.len() == 1 {
            result.insert("type".to_string(), types[0].clone());
            if accepts_null {
                nullable = true;
            }
        } else {
            result.remove("type");
            let mut members: Vec<Value> = types
                .iter()
                .map(|t| json!({ "type": t.clone() }))
                .collect();
            if accepts_null {
                members[0] = mark_nullable(&members[0], false);
            }
            if result.get("anyOf").is_none() {
                result.insert("anyOf".to_string(), Value::Array(members));
            } else {
                let all_of = result.get("allOf").and_then(|a| a.as_array()).cloned();
                let mut list = all_of.unwrap_or_default();
                list.push(json!({ "anyOf": members }));
                result.insert("allOf".to_string(), Value::Array(list));
            }
        }
    }

    for key in ["anyOf", "oneOf"] {
        let Some(Value::Array(members)) = result.get(key).cloned() else {
            continue;
        };
        if !members.iter().any(is_null_schema) {
            continue;
        }
        let kept: Vec<Value> = members.iter().filter(|m| !is_null_schema(m)).cloned().collect();
        result.remove(key);
        if kept.len() == 1 {
            // The node's own keywords (description, default, ...) take priority over the member's.
            let merged = merge_shallow(&kept[0], &Value::Object(result.clone()));
            result = merged.as_object().cloned().unwrap_or_default();
            nullable = true;
        } else if !kept.is_empty() {
            // `nullable` must sit next to a `type`, so it goes on the first member that has one. One
            // member is enough for anyOf, and for oneOf null has to match exactly one member.
            let typed = kept
                .iter()
                .position(|m| m.get("type").map(|t| t.is_string()).unwrap_or(false));
            let mut kept = kept;
            if let Some(idx) = typed {
                kept[idx] = mark_nullable(&kept[idx], true);
            } else {
                nullable = true;
            }
            result.insert(key.to_string(), Value::Array(kept));
        } else {
            nullable = true;
        }
    }

    if nullable {
        mark_nullable(&Value::Object(result), true)
    } else {
        Value::Object(result)
    }
}

/// Make a schema accept null in OpenAPI 3.0. `nullable` only counts next to a `type`, and an
/// `enum` accepts null only when it lists it. `extend_enum` is false when the schema's own enum
/// is meant to keep excluding null.
fn mark_nullable(schema: &Value, extend_enum: bool) -> Value {
    let mut marked = match schema {
        Value::Object(m) => m.clone(),
        _ => return schema.clone(),
    };
    let has_enum = marked.get("enum").map(|e| e.is_array()).unwrap_or(false);
    if has_enum && extend_enum {
        let includes_null = marked["enum"]
            .as_array()
            .map(|e| e.iter().any(|v| v.is_null()))
            .unwrap_or(false);
        if !includes_null {
            let mut e = marked["enum"].as_array().unwrap().clone();
            e.push(Value::Null);
            marked.insert("enum".to_string(), Value::Array(e));
        }
    }
    let type_is_string = marked.get("type").map(|t| t.is_string()).unwrap_or(false);
    if type_is_string || !has_enum {
        marked.insert("nullable".to_string(), Value::Bool(true));
    }
    Value::Object(marked)
}

/// Resolve the schema that decides how a value is converted. A `type` list or an
/// `anyOf`/`oneOf` union that has exactly one non-null member (the usual encoding of a
/// nullable parameter) is treated as that member; anything more ambiguous is left alone.
pub fn resolve_effective_schema(prop_def: &Value) -> Value {
    let obj = match prop_def.as_object() {
        Some(o) => o,
        None => return prop_def.clone(),
    };
    if let Some(Value::Array(types)) = obj.get("type") {
        let non_null: Vec<&Value> = types.iter().filter(|t| t.as_str() != Some("null")).collect();
        if non_null.len() == 1 {
            let mut out = obj.clone();
            out.insert("type".to_string(), non_null[0].clone());
            return Value::Object(out);
        }
        return prop_def.clone();
    }
    if obj.get("type").is_none() {
        for key in ["anyOf", "oneOf"] {
            let Some(Value::Array(members)) = obj.get(key) else {
                continue;
            };
            let non_null: Vec<&Value> = members
                .iter()
                .filter(|m| m.is_object() && m.get("type").and_then(|t| t.as_str()) != Some("null"))
                .collect();
            if non_null.len() == 1 {
                let mut rest = obj.clone();
                rest.remove(key);
                let merged = merge_shallow(&Value::Object(rest), non_null[0]);
                return resolve_effective_schema(&merged);
            }
        }
    }
    prop_def.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn to_oapi(v: Value) -> Value {
        to_openapi_schema(&v, &Map::new())
    }

    // ── type lists ──

    #[test]
    fn type_list_with_null_becomes_nullable() {
        assert_eq!(
            to_oapi(json!({"type": ["string", "null"]})),
            json!({"type": "string", "nullable": true})
        );
    }

    #[test]
    fn multi_type_list_with_null_becomes_any_of() {
        assert_eq!(
            to_oapi(json!({"type": ["string", "integer", "null"]})),
            json!({"anyOf": [
                {"type": "string", "nullable": true},
                {"type": "integer"}
            ]})
        );
    }

    #[test]
    fn multi_type_list_without_null_becomes_any_of() {
        assert_eq!(
            to_oapi(json!({"type": ["string", "integer"]})),
            json!({"anyOf": [{"type": "string"}, {"type": "integer"}]})
        );
    }

    #[test]
    fn null_only_type_list_drops_type() {
        assert_eq!(to_oapi(json!({"type": ["null"]})), json!({}));
    }

    // ── const / enum ──

    #[test]
    fn const_becomes_single_value_enum() {
        assert_eq!(
            to_oapi(json!({"const": "a"})),
            json!({"enum": ["a"]})
        );
    }

    #[test]
    fn nullable_type_list_extends_enum_with_null() {
        assert_eq!(
            to_oapi(json!({"type": ["string", "null"], "enum": ["a", "b"]})),
            json!({"type": "string", "enum": ["a", "b", null], "nullable": true})
        );
    }

    #[test]
    fn nullable_type_list_keeps_enum_that_already_has_null() {
        assert_eq!(
            to_oapi(json!({"type": ["string", "null"], "enum": ["a", null]})),
            json!({"type": "string", "enum": ["a", null], "nullable": true})
        );
    }

    #[test]
    fn non_nullable_enum_does_not_accept_null() {
        assert_eq!(
            to_oapi(json!({"type": "string", "enum": ["a"]})),
            json!({"type": "string", "enum": ["a"]})
        );
    }

    // ── anyOf / oneOf with null members ──

    #[test]
    fn any_of_single_member_with_null_merges_member() {
        assert_eq!(
            to_oapi(json!({"anyOf": [{"type": "string", "enum": ["a"]}, {"type": "null"}]})),
            json!({"type": "string", "enum": ["a", null], "nullable": true})
        );
    }

    #[test]
    fn any_of_const_member_with_null() {
        assert_eq!(
            to_oapi(json!({"anyOf": [{"const": "a"}, {"type": "null"}]})),
            json!({"enum": ["a", null]})
        );
    }

    #[test]
    fn any_of_several_members_nullable_on_first_typed() {
        assert_eq!(
            to_oapi(json!({"anyOf": [{"type": "string"}, {"type": "integer"}, {"type": "null"}]})),
            json!({"anyOf": [{"type": "string", "nullable": true}, {"type": "integer"}]})
        );
    }

    #[test]
    fn one_of_several_members_nullable_on_first_typed() {
        assert_eq!(
            to_oapi(json!({"oneOf": [{"type": "string"}, {"type": "integer"}, {"type": "null"}]})),
            json!({"oneOf": [{"type": "string", "nullable": true}, {"type": "integer"}]})
        );
    }

    #[test]
    fn any_of_only_null_members_becomes_nullable() {
        assert_eq!(
            to_oapi(json!({"anyOf": [{"type": "null"}]})),
            json!({"nullable": true})
        );
    }

    #[test]
    fn own_keywords_take_priority_over_merged_member() {
        assert_eq!(
            to_oapi(json!({"anyOf": [{"type": "string"}, {"type": "null"}], "description": "d"})),
            json!({"type": "string", "nullable": true, "description": "d"})
        );
    }

    // ── nested structures ──

    #[test]
    fn nullable_properties_and_array_items_are_converted() {
        assert_eq!(
            to_oapi(json!({
                "type": "object",
                "properties": {
                    "x": {"type": ["string", "integer", "null"]},
                    "y": {"anyOf": [{"type": "boolean"}, {"type": "null"}]}
                }
            })),
            json!({
                "type": "object",
                "properties": {
                    "x": {"anyOf": [{"type": "string", "nullable": true}, {"type": "integer"}]},
                    "y": {"type": "boolean", "nullable": true}
                }
            })
        );
        assert_eq!(
            to_oapi(json!({"type": "array", "items": {"type": ["integer", "null"]}})),
            json!({"type": "array", "items": {"type": "integer", "nullable": true}})
        );
    }

    #[test]
    fn arrays_and_scalars_pass_through() {
        assert_eq!(to_oapi(json!([{"type": ["string", "null"]}])), json!([{"type": "string", "nullable": true}]));
        assert_eq!(to_oapi(json!("plain")), json!("plain"));
        assert_eq!(to_oapi(json!(42)), json!(42));
        assert_eq!(to_oapi(json!(null)), json!(null));
    }

    // ── definitions / $ref ──

    #[test]
    fn collect_definitions_merges_both_keys() {
        let defs = collect_definitions(&json!({
            "$defs": {"A": {"type": "string"}},
            "definitions": {"B": {"type": "number"}}
        }));
        assert_eq!(defs.len(), 2);
        assert_eq!(defs.get("A"), Some(&json!({"type": "string"})));
        assert_eq!(defs.get("B"), Some(&json!({"type": "number"})));
    }

    #[test]
    fn unresolvable_ref_is_kept() {
        assert_eq!(
            to_oapi(json!({"$ref": "#/$defs/Missing"})),
            json!({"$ref": "#/$defs/Missing"})
        );
        assert_eq!(
            to_oapi(json!({"$ref": "https://example.com/schema.json"})),
            json!({"$ref": "https://example.com/schema.json"})
        );
    }

    #[test]
    fn resolvable_ref_is_inlined_with_nullable_conversion() {
        let mut defs = Map::new();
        defs.insert("Limit".to_string(), json!({"type": ["integer", "null"], "minimum": 1}));
        assert_eq!(
            to_openapi_schema(&json!({"$ref": "#/$defs/Limit"}), &defs),
            json!({"type": "integer", "nullable": true, "minimum": 1})
        );
    }

    #[test]
    fn recursive_ref_becomes_empty_schema() {
        let mut defs = Map::new();
        defs.insert(
            "Node".to_string(),
            json!({"type": "object", "properties": {"child": {"$ref": "#/$defs/Node"}}}),
        );
        let out = to_openapi_schema(&json!({"$ref": "#/$defs/Node"}), &defs);
        // Outer node inlines; the inner recursive reference collapses to {}.
        assert_eq!(out, json!({"type": "object", "properties": {"child": {}}}));
    }

    #[test]
    fn ref_sibling_keywords_win_over_definition() {
        let mut defs = Map::new();
        defs.insert("A".to_string(), json!({"type": "string", "description": "from def"}));
        assert_eq!(
            to_openapi_schema(&json!({"$ref": "#/$defs/A", "description": "own"}), &defs),
            json!({"type": "string", "description": "own"})
        );
    }

    // ── resolve_effective_schema ──

    #[test]
    fn type_list_resolves_to_single_non_null() {
        assert_eq!(
            resolve_effective_schema(&json!({"type": ["integer", "null"]})),
            json!({"type": "integer"})
        );
    }

    #[test]
    fn ambiguous_type_list_is_left_alone() {
        let def = json!({"type": ["string", "integer"]});
        assert_eq!(resolve_effective_schema(&def), def);
    }

    #[test]
    fn any_of_single_non_null_member_resolves() {
        assert_eq!(
            resolve_effective_schema(&json!({"anyOf": [{"type": "integer"}, {"type": "null"}], "description": "d"})),
            json!({"type": "integer", "description": "d"})
        );
    }

    #[test]
    fn one_of_single_non_null_member_resolves_recursively() {
        assert_eq!(
            resolve_effective_schema(&json!({"oneOf": [{"anyOf": [{"type": "boolean"}, {"type": "null"}]}]})),
            json!({"type": "boolean"})
        );
    }

    #[test]
    fn ambiguous_union_is_left_alone() {
        let def = json!({"anyOf": [{"type": "string"}, {"type": "integer"}]});
        assert_eq!(resolve_effective_schema(&def), def);
    }

    #[test]
    fn definitions_prefix_ref_is_inlined() {
        let mut defs = Map::new();
        defs.insert("A".to_string(), json!({"type": ["string", "null"]}));
        assert_eq!(
            to_openapi_schema(&json!({"$ref": "#/definitions/A"}), &defs),
            json!({"type": "string", "nullable": true})
        );
    }

    #[test]
    fn additional_properties_and_not_are_converted() {
        assert_eq!(
            to_oapi(json!({"type": "object", "additionalProperties": {"type": ["string", "null"]}})),
            json!({"type": "object", "additionalProperties": {"type": "string", "nullable": true}})
        );
        assert_eq!(
            to_oapi(json!({"not": {"type": ["string", "null"]}})),
            json!({"not": {"type": "string", "nullable": true}})
        );
    }

    #[test]
    fn all_of_members_are_converted() {
        assert_eq!(
            to_oapi(json!({"allOf": [{"type": ["string", "null"]}]})),
            json!({"allOf": [{"type": "string", "nullable": true}]})
        );
    }

    #[test]
    fn const_is_kept_when_enum_already_exists() {
        assert_eq!(
            to_oapi(json!({"const": "a", "enum": ["a", "b"]})),
            json!({"const": "a", "enum": ["a", "b"]})
        );
    }

    #[test]
    fn type_list_with_existing_any_of_merges_via_all_of() {
        assert_eq!(
            to_oapi(json!({"type": ["string", "integer", "null"], "anyOf": [{"type": "boolean"}]})),
            json!({"anyOf": [{"type": "boolean"}], "allOf": [
                {"anyOf": [{"type": "string", "nullable": true}, {"type": "integer"}]}
            ]})
        );
    }

    #[test]
    fn resolve_null_only_type_list_is_left_alone() {
        let def = json!({"type": ["null"]});
        assert_eq!(resolve_effective_schema(&def), def);
    }

    #[test]
    fn resolve_all_null_union_is_left_alone() {
        let def = json!({"anyOf": [{"type": "null"}]});
        assert_eq!(resolve_effective_schema(&def), def);
    }

    #[test]
    fn non_string_ref_is_dropped_like_origin() {
        // Origin destructures $ref out of rest regardless of type; a non-string
        // $ref never enters the reference branch and is not kept.
        assert_eq!(to_oapi(json!({"$ref": 1})), json!({}));
    }

    #[test]
    fn plain_schema_passes_through() {
        let def = json!({"type": "string"});
        assert_eq!(resolve_effective_schema(&def), def);
        assert_eq!(resolve_effective_schema(&json!("x")), json!("x"));
    }
}
