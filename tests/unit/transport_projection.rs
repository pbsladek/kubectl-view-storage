#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use serde_json::json;

struct Budget(usize);
impl Checkpoint for Budget {
    type Error = ();
    fn step(&mut self) -> Result<(), ()> {
        self.0 = self.0.checked_sub(1).ok_or(())?;
        Ok(())
    }
}
fn fixture() -> Value {
    serde_json::from_str::<Value>(include_str!("../../examples/contract/healthy/input.json"))
        .unwrap()["kubernetes"]["pvcs"][0]
        .clone()
}

#[test]
fn checked_projection_matches_serde_for_present_missing_null_and_invalid_fields() {
    let original = fixture();
    let expected: Object = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(
        Object::from_value_checked(&original, &mut Budget(usize::MAX)).unwrap(),
        expected
    );
    for path in [
        "/metadata/name",
        "/metadata/uid",
        "/metadata/namespace",
        "/metadata/creationTimestamp",
        "/metadata/deletionTimestamp",
        "/metadata/labels",
        "/metadata/ownerReferences",
        "/spec",
        "/status",
        "/provisioner",
        "/volumeBindingMode",
    ] {
        for value in [
            Value::Null,
            json!(true),
            json!(7),
            json!("text"),
            json!([]),
            json!({}),
            json!({"x":"v"}),
        ] {
            let mut changed = original.clone();
            let (parent, key) = path.rsplit_once('/').unwrap();
            changed
                .pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert(key.into(), value.clone());
            let reference = serde_json::from_value::<Object>(changed.clone());
            let actual = Object::from_value_checked(&changed, &mut Budget(usize::MAX));
            assert_eq!(actual.is_ok(), reference.is_ok(), "{path}: {value}");
            if let Ok(expected) = reference {
                assert_eq!(actual.unwrap(), expected, "{path}: {value}");
            }
        }
    }
    for value in [
        json!({}),
        json!({"metadata":{}}),
        json!({"metadata":{"name":"only-name"}}),
    ] {
        let reference = serde_json::from_value::<Object>(value.clone());
        let actual = Object::from_value_checked(&value, &mut Budget(usize::MAX));
        assert_eq!(actual.is_ok(), reference.is_ok());
        if let Ok(expected) = reference {
            assert_eq!(actual.unwrap(), expected);
        }
    }
    // Kubernetes API objects and metadata are maps; positional struct-array
    // encodings accepted by some generic deserializers are not API objects.
    for value in [json!([]), json!({"metadata":[]}), Value::Null] {
        assert!(Object::from_value_checked(&value, &mut Budget(usize::MAX)).is_err());
    }
}

#[test]
fn large_known_containers_are_interruptible_and_unknown_fields_are_ignored() {
    let original = fixture();
    for path in [
        "/metadata/labels",
        "/metadata/ownerReferences",
        "/spec/unknown",
    ] {
        let mut value = original.clone();
        let (parent, key) = path.rsplit_once('/').unwrap();
        let content = if key == "labels" {
            Value::Object(
                (0..1000)
                    .map(|i| (format!("key-{i}"), json!("value")))
                    .collect(),
            )
        } else {
            json!((0..1000).collect::<Vec<_>>())
        };
        value
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(key.into(), content);
        assert!(matches!(
            Object::from_value_checked(&value, &mut Budget(64)),
            Err(CheckedError::Interrupted(()))
        ));
    }
    let mut value = original.clone();
    value["unknown"] = json!((0..10000).collect::<Vec<_>>());
    let projected = Object::from_value_checked(&value, &mut Budget(100)).unwrap();
    assert_eq!(
        projected,
        serde_json::from_value::<Object>(original).unwrap()
    );
}

#[test]
fn string_and_raw_json_copying_preserve_unicode_and_honor_byte_boundaries() {
    let text = "界β".repeat(work::BYTE_CHUNK);
    assert_eq!(
        work::clone_string(&text, &mut Budget(usize::MAX)).unwrap(),
        text
    );
    assert!(work::clone_string(&text, &mut Budget(1)).is_err());
    let raw = json!({text.clone():[text,true,null,42]});
    assert_eq!(
        work::clone_json(&raw, &mut Budget(usize::MAX)).unwrap(),
        raw
    );
    assert!(work::clone_json(&raw, &mut Budget(2)).is_err());
    let mut object: Object = serde_json::from_value(fixture()).unwrap();
    object.metadata.name = "x".repeat(10000);
    assert!(
        object
            .valid_api_object_checked(
                "persistentvolumeclaims",
                &ApiScope::Namespace("demo".into()),
                &mut Budget(64)
            )
            .is_err()
    );
    object.metadata.name = "valid".into();
    object.metadata.uid = Some("x".repeat(10000));
    assert!(
        object
            .valid_api_object_checked(
                "persistentvolumeclaims",
                &ApiScope::Namespace("demo".into()),
                &mut Budget(64)
            )
            .is_err()
    );
}
