//! Executor-neutral native composition diff, identity namespace and path accounting.
//! These values and algorithms carry no grant or callback execution authority.
use crate::NativePatchOperation;
use serde_json::Value;
use std::collections::BTreeMap;

pub const MAX_COMPONENT_NODES: usize = 4096;
pub const MAX_TREE_DEPTH: usize = 128;
pub const MAX_JSON_BYTES: usize = 1024 * 1024;

#[derive(Clone, Default)]
pub struct MountPatchAuthority {
    /// Package-local handler slot -> stable expanded event token.
    pub slots: BTreeMap<String, u64>,
    /// Package-local native identity -> retained composition expansion paths
    /// contained by that native subtree.
    pub splices: BTreeMap<String, std::collections::BTreeSet<String>>,
    /// Exact unexpanded native identity -> composition traversal path. Keeping
    /// this mapping avoids inventing a new mount namespace on parent replacement.
    pub paths: BTreeMap<String, String>,
}

pub fn composition_child_identity(value: &Value, index: usize) -> String {
    value.get("key").map_or_else(
        || format!("#{index}"),
        |key| {
            format!(
                "@{}",
                key.to_string()
                    .bytes()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            )
        },
    )
}

pub fn native_slot_in_subtree(slot: &str, target: &str) -> bool {
    slot.strip_prefix(target).is_some_and(|suffix| {
        suffix.is_empty() || suffix.starts_with('/') || suffix.starts_with(':')
    })
}

impl MountPatchAuthority {
    pub fn forget_paths(&mut self, target: &str) {
        let prefix = format!("{target}/");
        self.paths
            .retain(|id, _| id != target && !id.starts_with(&prefix));
    }

    pub fn record_paths(&mut self, value: &Value, path: &str, depth: usize) -> Result<(), String> {
        if depth > MAX_TREE_DEPTH {
            return Err("component tree exceeds depth limit".into());
        }
        if let Some(id) = value.get("__nativeId").and_then(Value::as_str) {
            let local = id.rsplit_once("::").map_or(id, |(_, local)| local);
            self.paths.insert(local.to_owned(), path.to_owned());
            if self.paths.len() > MAX_COMPONENT_NODES {
                return Err("component path index exceeds node limit".into());
            }
        }
        match value {
            Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    self.record_paths(
                        value,
                        &format!("{path}/{}", composition_child_identity(value, index)),
                        depth + 1,
                    )?;
                }
            }
            Value::Object(object) => {
                for (key, value) in object {
                    if key != "__handlerSlots" {
                        self.record_paths(value, &format!("{path}/{key}"), depth + 1)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn find_unique_native_prefix<'a>(
    value: &'a Value,
    prefix: &str,
) -> Result<Option<&'a Value>, String> {
    fn visit<'a>(
        value: &'a Value,
        prefix: &str,
        found: &mut Option<&'a Value>,
    ) -> Result<(), String> {
        if value
            .get("__nativeId")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with(prefix))
        {
            if found.replace(value).is_some() {
                return Err("ambiguous duplicate retained ownership path".into());
            }
            return Ok(());
        }
        match value {
            Value::Object(object) => {
                for value in object.values() {
                    visit(value, prefix, found)?;
                }
            }
            Value::Array(values) => {
                for value in values {
                    visit(value, prefix, found)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut found = None;
    visit(value, prefix, &mut found)?;
    Ok(found)
}

pub fn find_native_id<'a>(value: &'a Value, target: &str) -> Option<&'a Value> {
    if value.get("__nativeId").and_then(Value::as_str) == Some(target) {
        return Some(value);
    }
    match value {
        Value::Object(object) => object
            .values()
            .find_map(|value| find_native_id(value, target)),
        Value::Array(values) => values
            .iter()
            .find_map(|value| find_native_id(value, target)),
        _ => None,
    }
}

pub fn diff_native_subtree(
    before: &Value,
    after: &Value,
    operations: &mut Vec<NativePatchOperation>,
    visited: &mut u64,
) -> Result<(), String> {
    *visited = visited.saturating_add(1);
    if before == after {
        return Ok(());
    }
    let Some(target) = after.get("__nativeId").and_then(Value::as_str) else {
        return Err("incremental native candidate has no identity".into());
    };
    if before.get("__nativeId").and_then(Value::as_str) != Some(target)
        || before.get("kind") != after.get("kind")
    {
        operations.push(NativePatchOperation::ReplaceSubtree {
            target: before
                .get("__nativeId")
                .and_then(Value::as_str)
                .ok_or("replaced native subtree has no identity")?
                .to_owned(),
            node: after.clone(),
        });
        return Ok(());
    }
    let before_object = before.as_object().ok_or("native node is not an object")?;
    let after_object = after.as_object().ok_or("native node is not an object")?;
    for (property, value) in after_object {
        if matches!(
            property.as_str(),
            "children" | "__nativeId" | "__handlerSlots"
        ) || is_action(property)
        {
            continue;
        }
        if before_object.get(property) != Some(value) {
            if value.is_object() || value.is_array() {
                operations.push(NativePatchOperation::ReplaceSubtree {
                    target: target.to_owned(),
                    node: after.clone(),
                });
                return Ok(());
            }
            operations.push(NativePatchOperation::SetPrimitive {
                target: target.to_owned(),
                property: property.clone(),
                value: value.clone(),
            });
        }
    }
    let before_slots = before_object
        .get("__handlerSlots")
        .and_then(Value::as_object);
    let after_slots = after_object
        .get("__handlerSlots")
        .and_then(Value::as_object);
    if before_slots.map(|slots| slots.keys().collect::<Vec<_>>())
        != after_slots.map(|slots| slots.keys().collect::<Vec<_>>())
    {
        operations.push(NativePatchOperation::ReplaceSubtree {
            target: target.to_owned(),
            node: after.clone(),
        });
        return Ok(());
    }
    if let Some(slots) = after_slots {
        if slots.keys().any(|event| {
            before_object
                .get(event)
                .is_some_and(|value| !value.is_null())
                != after_object
                    .get(event)
                    .is_some_and(|value| !value.is_null())
        }) {
            operations.push(NativePatchOperation::ReplaceSubtree {
                target: target.to_owned(),
                node: after.clone(),
            });
            return Ok(());
        }
        for (event, slot) in slots {
            let action = after_object.get(event).and_then(Value::as_u64);
            if action != before_object.get(event).and_then(Value::as_u64) {
                operations.push(NativePatchOperation::ReplaceHandlerSlot {
                    slot: slot
                        .as_str()
                        .ok_or("native handler slot must be a string")?
                        .to_owned(),
                    action: usize::try_from(action.ok_or("native handler action disappeared")?)
                        .map_err(|_| "native handler action exceeds host range")?,
                });
            }
        }
    }
    let before_children = before_object.get("children").and_then(Value::as_array);
    let after_children = after_object.get("children").and_then(Value::as_array);
    match (before_children, after_children) {
        (Some(before_children), Some(after_children))
            if before_children.iter().all(|child| !child.is_object())
                && after_children.iter().all(|child| !child.is_object()) =>
        {
            if before_children != after_children {
                operations.push(NativePatchOperation::SetPrimitive {
                    target: target.to_owned(),
                    property: "children".into(),
                    value: Value::Array(after_children.clone()),
                });
            }
        }
        (Some(before_children), Some(after_children))
            if before_children.iter().chain(after_children).all(|child| {
                child.get("key").and_then(Value::as_str).is_some()
                    && child.get("__nativeId").and_then(Value::as_str).is_some()
            }) =>
        {
            let mut current = before_children.clone();
            for index in (0..current.len()).rev() {
                let id = current[index]["__nativeId"].as_str().unwrap();
                if !after_children.iter().any(|child| child["__nativeId"] == id) {
                    operations.push(NativePatchOperation::RemoveChild {
                        parent: target.to_owned(),
                        key: current[index]["key"].as_str().unwrap().to_owned(),
                        child_id: id.to_owned(),
                        index,
                    });
                    current.remove(index);
                }
            }
            for (index, desired) in after_children.iter().enumerate() {
                let desired_id = desired["__nativeId"].as_str().unwrap();
                if let Some(from) = current
                    .iter()
                    .position(|child| child["__nativeId"] == desired_id)
                {
                    if from != index {
                        operations.push(NativePatchOperation::MoveChild {
                            parent: target.to_owned(),
                            key: desired["key"].as_str().unwrap().to_owned(),
                            child_id: desired_id.to_owned(),
                            from,
                            to: index,
                        });
                        let child = current.remove(from);
                        current.insert(index, child);
                    }
                    diff_native_subtree(&current[index], desired, operations, visited)?;
                } else {
                    operations.push(NativePatchOperation::InsertChild {
                        parent: target.to_owned(),
                        key: desired["key"].as_str().unwrap().to_owned(),
                        child_id: desired_id.to_owned(),
                        index,
                        node: desired.clone(),
                    });
                    current.insert(index, desired.clone());
                }
            }
        }
        (Some(before_children), Some(after_children))
            if before_children.len() == after_children.len() =>
        {
            // Positional children have no structural operation that can change
            // their native identity in place. Replace the nearest stable
            // parent boundary instead of emitting an invalid child replacement
            // whose target names the old node while its payload names the new
            // one.
            if before_children
                .iter()
                .zip(after_children)
                .any(|(before, after)| {
                    before.get("__nativeId") != after.get("__nativeId")
                        || before.get("kind") != after.get("kind")
                })
            {
                operations.push(NativePatchOperation::ReplaceSubtree {
                    target: target.to_owned(),
                    node: after.clone(),
                });
                return Ok(());
            }
            for (before, after) in before_children.iter().zip(after_children) {
                diff_native_subtree(before, after, operations, visited)?;
            }
        }
        _ => operations.push(NativePatchOperation::ReplaceSubtree {
            target: target.to_owned(),
            node: after.clone(),
        }),
    }
    Ok(())
}

pub fn normalize_atomic_operations(
    operations: Vec<NativePatchOperation>,
) -> Vec<NativePatchOperation> {
    fn target(operation: &NativePatchOperation) -> &str {
        match operation {
            NativePatchOperation::SetPrimitive { target, .. }
            | NativePatchOperation::ReplaceSubtree { target, .. } => target,
            NativePatchOperation::ReplaceHandlerSlot { slot, .. } => slot,
            NativePatchOperation::InsertChild { parent, .. }
            | NativePatchOperation::RemoveChild { parent, .. }
            | NativePatchOperation::MoveChild { parent, .. } => parent,
        }
    }
    fn below(candidate: &str, ancestor: &str) -> bool {
        candidate
            .strip_prefix(ancestor)
            .is_some_and(|suffix| suffix.starts_with('/') || suffix.starts_with("::"))
    }
    fn at_or_below(candidate: &str, ancestor: &str) -> bool {
        candidate == ancestor || below(candidate, ancestor)
    }
    let mut normalized = Vec::with_capacity(operations.len());
    let mut covered = Vec::<String>::new();
    for operation in operations {
        let operation_target = target(&operation);
        let replaces_subtree = matches!(operation, NativePatchOperation::ReplaceSubtree { .. });
        if covered.iter().any(|ancestor| {
            below(operation_target, ancestor) || (!replaces_subtree && operation_target == ancestor)
        }) {
            continue;
        }
        if let NativePatchOperation::ReplaceSubtree {
            target: replaced_target,
            ..
        } = &operation
        {
            normalized.retain(|prior| !at_or_below(target(prior), replaced_target));
            covered.retain(|prior| !at_or_below(prior, replaced_target));
            covered.push(replaced_target.clone());
        }
        if let NativePatchOperation::RemoveChild { child_id, .. } = &operation {
            normalized.retain(|prior| !at_or_below(target(prior), child_id));
            covered.retain(|prior| !at_or_below(prior, child_id));
            covered.push(child_id.clone());
        }
        normalized.push(operation);
    }
    normalized
}

pub fn is_action(key: &str) -> bool {
    matches!(
        key,
        "action"
            | "__callbackAction"
            | "contextAction"
            | "dragAction"
            | "dropAction"
            | "focusAction"
            | "blurAction"
            | "selectAction"
            | "moveAction"
            | "fileAction"
            | "closeAction"
            | "escapeAction"
            | "submitAction"
    )
}

pub fn bounded_json(value: &Value) -> Result<(), String> {
    // Count the exact encoded size without allocating a second full snapshot.
    struct Budget(usize);
    impl std::io::Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.0 {
                return Err(std::io::Error::other("composition JSON exceeds size limit"));
            }
            self.0 -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Budget(MAX_JSON_BYTES), value)
        .map_err(|_| "composition JSON exceeds size limit".into())
}

pub fn namespace_native_metadata(
    value: &mut Value,
    prefix: &str,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_TREE_DEPTH || prefix.len() > 512 {
        return Err("native identity namespace exceeds limit".into());
    }
    match value {
        Value::Array(values) => {
            for value in values {
                namespace_native_metadata(value, prefix, depth + 1)?;
            }
        }
        Value::Object(object) => {
            if let Some(id) = object.get_mut("__nativeId") {
                let identity = id
                    .as_str()
                    .ok_or("native node identity must be a string")?
                    .to_owned();
                if identity.is_empty() || identity.len() > 512 {
                    return Err("native node identity exceeds limit".into());
                }
                *id = Value::String(format!("{prefix}::{identity}"));
            }
            if let Some(slots) = object.get_mut("__handlerSlots") {
                let slots = slots
                    .as_object_mut()
                    .ok_or("native handler slots must be an object")?;
                for slot in slots.values_mut() {
                    let value = slot
                        .as_str()
                        .ok_or("native handler slot must be a string")?;
                    if value.is_empty() || value.len() > 640 {
                        return Err("native handler slot exceeds limit".into());
                    }
                    *slot = Value::String(format!("{prefix}::{value}"));
                }
            }
            for child in object.values_mut() {
                namespace_native_metadata(child, prefix, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn native_path_retirement_is_bounded_and_does_not_match_key_prefix_siblings() {
        let mut authority = MountPatchAuthority::default();
        authority
            .record_paths(
                &json!({"__nativeId":"root/@row-10"}),
                "root/children/sibling",
                0,
            )
            .unwrap();
        for iteration in 0..1000 {
            let id = format!("root/@row-1/child-{iteration}");
            authority.record_paths(&json!({"__nativeId":"root/@row-1","children":[{"__nativeId":id,"key":"child"}]}),"root/children/row",0).unwrap();
            assert_eq!(authority.paths.len(), 3);
            assert!(native_slot_in_subtree("root/@row-1:action", "root/@row-1"));
            assert!(native_slot_in_subtree(
                &format!("{id}:action"),
                "root/@row-1"
            ));
            assert!(!native_slot_in_subtree(
                "root/@row-10:action",
                "root/@row-1"
            ));
            authority.forget_paths("root/@row-1");
            assert_eq!(authority.paths.len(), 1);
            assert!(authority.paths.contains_key("root/@row-10"));
        }
    }

    #[test]
    fn native_diff_keeps_keyed_and_handler_only_updates_typed() {
        let before = serde_json::json!({
            "kind":"Column","__nativeId":"root","children":[
                {"kind":"Button","key":"a","__nativeId":"a","__handlerSlots":{"action":"a::action"},"action":0,"children":["a"]},
                {"kind":"Text","key":"b","__nativeId":"b","children":["b"]}
            ]
        });
        let after = serde_json::json!({
            "kind":"Column","__nativeId":"root","children":[
                {"kind":"Text","key":"b","__nativeId":"b","children":["b"]},
                {"kind":"Button","key":"a","__nativeId":"a","__handlerSlots":{"action":"a::action"},"action":9,"children":["a"]},
                {"kind":"Text","key":"c","__nativeId":"c","children":["c"]}
            ]
        });
        let mut operations = Vec::new();
        let mut visited = 0;
        diff_native_subtree(&before, &after, &mut operations, &mut visited).unwrap();
        assert!(operations.iter().any(|operation| matches!(operation, NativePatchOperation::MoveChild { child_id, .. } if child_id == "b")));
        assert!(operations.iter().any(|operation| matches!(operation, NativePatchOperation::InsertChild { child_id, .. } if child_id == "c")));
        assert!(operations.iter().any(|operation| matches!(operation, NativePatchOperation::ReplaceHandlerSlot { slot, action } if slot == "a::action" && *action == 9)));
        assert!(!operations.iter().any(|operation| matches!(operation, NativePatchOperation::ReplaceSubtree { target, .. } if target == "root")));
        assert!(visited >= 2);
    }

    #[test]
    fn native_diff_replaces_only_the_node_when_its_handler_slots_change_shape() {
        let before = serde_json::json!({
            "kind":"window","__nativeId":"root","__handlerSlots":{"escapeAction":"root:escapeAction"},"escapeAction":0,"children":[]
        });
        let after = serde_json::json!({
            "kind":"window","__nativeId":"root","__handlerSlots":{"escapeAction":"root:escapeAction","submitAction":"root:submitAction"},"escapeAction":0,"submitAction":1,"children":[]
        });
        let mut operations = Vec::new();
        let mut visited = 0;
        diff_native_subtree(&before, &after, &mut operations, &mut visited).unwrap();
        assert_eq!(operations.len(), 1);
        assert!(matches!(
            &operations[0],
            NativePatchOperation::ReplaceSubtree { target, node }
                if target == "root" && node["submitAction"] == 1
        ));
    }

    #[test]
    fn native_diff_replaces_stable_parent_when_positional_child_identity_changes() {
        let before = serde_json::json!({
            "kind":"Column","__nativeId":"root","children":[
                {"kind":"Text","__nativeId":"root/old","children":["old"]}
            ]
        });
        let after = serde_json::json!({
            "kind":"Column","__nativeId":"root","children":[
                {"kind":"Button","__nativeId":"root/new","children":["new"]}
            ]
        });
        let mut operations = Vec::new();
        let mut visited = 0;
        diff_native_subtree(&before, &after, &mut operations, &mut visited).unwrap();
        assert_eq!(
            operations,
            vec![NativePatchOperation::ReplaceSubtree {
                target: "root".into(),
                node: after,
            }]
        );
        assert_eq!(visited, 1);
    }

    #[test]
    fn atomic_patch_suppresses_work_below_replaced_ancestor() {
        let replacement = serde_json::json!({
            "kind":"Column","__nativeId":"root/owner","children":[
                {"kind":"Text","key":"fresh","__nativeId":"root/owner/@fresh","children":["cold"]}
            ]
        });
        let operations = normalize_atomic_operations(vec![
            NativePatchOperation::ReplaceSubtree {
                target: "root/owner".into(),
                node: replacement.clone(),
            },
            NativePatchOperation::SetPrimitive {
                target: "root/owner/@retired".into(),
                property: "children".into(),
                value: serde_json::json!(["stale"]),
            },
        ]);
        assert_eq!(
            operations,
            vec![NativePatchOperation::ReplaceSubtree {
                target: "root/owner".into(),
                node: replacement,
            }]
        );
    }

    #[test]
    fn atomic_patch_replacement_supersedes_work_on_the_same_target() {
        let replacement = serde_json::json!({
            "kind":"Column","__nativeId":"root/owner","className":"fresh","children":[]
        });
        let operations = normalize_atomic_operations(vec![
            NativePatchOperation::SetPrimitive {
                target: "root/owner".into(),
                property: "className".into(),
                value: serde_json::json!("stale"),
            },
            NativePatchOperation::ReplaceSubtree {
                target: "root/owner".into(),
                node: replacement.clone(),
            },
            NativePatchOperation::SetPrimitive {
                target: "root/owner".into(),
                property: "className".into(),
                value: serde_json::json!("also-stale"),
            },
        ]);
        assert_eq!(
            operations,
            vec![NativePatchOperation::ReplaceSubtree {
                target: "root/owner".into(),
                node: replacement,
            }]
        );
    }
}
