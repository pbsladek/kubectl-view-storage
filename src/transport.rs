//! Kubernetes transport boundary; optional API fields never default counters to zero.
use crate::work::{self, CheckedError, Checkpoint, Unbounded};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{self, BufReader, Read};

pub const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_JSON_DEPTH: usize = 128;

/// Request scope is separate from its report label: `cluster` is also a valid
/// namespace name and must never act as a wildcard in object validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiScope {
    Cluster,
    Namespace(String),
}
impl ApiScope {
    pub fn from_namespace(namespace: Option<&str>) -> Self {
        match namespace {
            Some(namespace) => Self::Namespace(namespace.to_owned()),
            None => Self::Cluster,
        }
    }
    pub fn label(&self) -> &str {
        match self {
            Self::Cluster => "cluster",
            Self::Namespace(namespace) => namespace,
        }
    }
    fn accepts_namespace(&self, namespace: &str) -> bool {
        match self {
            Self::Cluster => true,
            Self::Namespace(expected) => namespace == expected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub name: String,
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub namespace: Option<String>,
    #[serde(default)]
    pub creation_timestamp: Option<String>,
    #[serde(default)]
    pub deletion_timestamp: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub owner_references: Vec<Value>,
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Object {
    pub metadata: Metadata,
    #[serde(default)]
    pub spec: Value,
    #[serde(default)]
    pub status: Value,
    #[serde(default)]
    pub provisioner: Option<String>,
    #[serde(rename = "volumeBindingMode", default)]
    pub volume_binding_mode: Option<String>,
}
impl Object {
    pub fn valid_api_object(&self, resource: &str, scope: &ApiScope) -> bool {
        self.valid_api_object_checked(resource, scope, &mut Unbounded)
            .unwrap_or_else(|never| match never {})
    }
    pub(crate) fn valid_api_object_checked<C: Checkpoint>(
        &self,
        resource: &str,
        scope: &ApiScope,
        control: &mut C,
    ) -> Result<bool, C::Error> {
        if (!self.spec.is_object() && !self.spec.is_null())
            || (!self.status.is_object() && !self.status.is_null())
        {
            return Ok(false);
        }
        if self
            .status
            .get("phase")
            .is_some_and(|phase| !phase.is_string() && !phase.is_null())
        {
            return Ok(false);
        }
        if !valid_text(&self.metadata.name, control)? {
            return Ok(false);
        }
        if resource == "pods"
            && (self
                .spec
                .get("nodeName")
                .is_some_and(|value| !value.is_string() && !value.is_null())
                || self
                    .status
                    .get("phase")
                    .is_some_and(|value| !value.is_string() && !value.is_null())
                || self
                    .metadata
                    .deletion_timestamp
                    .as_ref()
                    .is_some_and(|value| value.parse::<jiff::Timestamp>().is_err()))
        {
            return Ok(false);
        }
        let namespaced = matches!(resource, "persistentvolumeclaims" | "pods");
        if matches!(
            resource,
            "persistentvolumeclaims" | "persistentvolumes" | "pods"
        ) {
            let Some(uid) = self.metadata.uid.as_deref() else {
                return Ok(false);
            };
            if !crate::model::valid_uid_checked(uid, control)? {
                return Ok(false);
            }
        }
        if namespaced {
            let Some(namespace) = self.metadata.namespace.as_deref() else {
                return Ok(false);
            };
            if !valid_text(namespace, control)? || !scope.accepts_namespace(namespace) {
                return Ok(false);
            }
        }
        Ok(true)
    }
    /// Project only supported fields. The independent Serde derive remains
    /// useful for fixtures; network documents use checked, borrowed traversal.
    pub(crate) fn from_value_checked<C: Checkpoint>(
        value: &Value,
        control: &mut C,
    ) -> Result<Self, CheckedError<C::Error>> {
        let object = value.as_object().ok_or("invalid_api_object")?;
        let metadata = object
            .get("metadata")
            .and_then(Value::as_object)
            .ok_or("invalid_api_object")?;
        let name = metadata
            .get("name")
            .and_then(Value::as_str)
            .ok_or("invalid_api_object")?;
        let mut labels = BTreeMap::new();
        if let Some(raw) = metadata.get("labels") {
            for (key, value) in raw.as_object().ok_or("invalid_api_object")? {
                control.checked()?;
                let value = value.as_str().ok_or("invalid_api_object")?;
                labels.insert(
                    work::clone_string(key, control).map_err(CheckedError::Interrupted)?,
                    work::clone_string(value, control).map_err(CheckedError::Interrupted)?,
                );
            }
        }
        let mut owner_references = vec![];
        if let Some(raw) = metadata.get("ownerReferences") {
            for owner in raw.as_array().ok_or("invalid_api_object")? {
                control.checked()?;
                owner_references
                    .push(work::clone_json(owner, control).map_err(CheckedError::Interrupted)?);
            }
        }
        Ok(Self {
            metadata: Metadata {
                name: work::clone_string(name, control).map_err(CheckedError::Interrupted)?,
                uid: optional_string(metadata.get("uid"), control)?,
                namespace: optional_string(metadata.get("namespace"), control)?,
                creation_timestamp: optional_string(metadata.get("creationTimestamp"), control)?,
                deletion_timestamp: optional_string(metadata.get("deletionTimestamp"), control)?,
                labels,
                owner_references,
            },
            spec: work::clone_json(object.get("spec").unwrap_or(&Value::Null), control)
                .map_err(CheckedError::Interrupted)?,
            status: work::clone_json(object.get("status").unwrap_or(&Value::Null), control)
                .map_err(CheckedError::Interrupted)?,
            provisioner: optional_string(object.get("provisioner"), control)?,
            volume_binding_mode: optional_string(object.get("volumeBindingMode"), control)?,
        })
    }
    pub fn uid(&self) -> Result<&str, String> {
        self.metadata
            .uid
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("missing UID for {}", self.metadata.name))
    }
    pub fn namespace(&self) -> Result<&str, String> {
        self.metadata
            .namespace
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("missing namespace for {}", self.metadata.name))
    }
}

fn valid_text<C: Checkpoint>(value: &str, control: &mut C) -> Result<bool, C::Error> {
    if value.is_empty() {
        return Ok(false);
    }
    for ch in value.chars() {
        control.step()?;
        if ch.is_control() {
            return Ok(false);
        }
    }
    Ok(true)
}
fn optional_string<C: Checkpoint>(
    value: Option<&Value>,
    control: &mut C,
) -> Result<Option<String>, CheckedError<C::Error>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => work::clone_string(value, control)
            .map(Some)
            .map_err(CheckedError::Interrupted),
        _ => Err("invalid_api_object".into()),
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Inventory {
    #[serde(default)]
    pub pvcs: Vec<Object>,
    #[serde(default)]
    pub pvs: Vec<Object>,
    #[serde(default)]
    pub pods: Vec<Object>,
    #[serde(default)]
    pub storage_classes: Vec<Object>,
}

/// A lexical guard before Serde: strings do not contribute nesting and escaped
/// quotes cannot hide deep containers. Serde remains the JSON syntax authority.
pub fn check_json_limits(input: &[u8]) -> Result<(), String> {
    check_json_limits_checked(input, &mut Unbounded).map_err(CheckedError::into_message)
}
fn check_json_limits_checked<C: Checkpoint>(
    input: &[u8],
    control: &mut C,
) -> Result<(), CheckedError<C::Error>> {
    if input.len() > MAX_RESPONSE_BYTES {
        return Err("response_too_large".into());
    }
    let (mut depth, mut string, mut escape, mut scalar) = (0usize, false, false, 0usize);
    for (index, &byte) in input.iter().enumerate() {
        if index % DECODE_CHUNK == 0 {
            control.boundary().map_err(CheckedError::Interrupted)?;
        }
        if string {
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == b'"' {
                string = false;
            }
            continue;
        }
        match byte {
            b'"' => {
                string = true;
                scalar = 0;
            }
            b'[' | b'{' => {
                depth += 1;
                scalar = 0;
                if depth > MAX_JSON_DEPTH {
                    return Err("json_too_deep".into());
                }
            }
            b']' | b'}' => {
                depth = depth.saturating_sub(1);
                scalar = 0;
            }
            b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E' => {
                scalar += 1;
                if scalar > crate::qty::MAX_SCALAR_BYTES {
                    return Err("numeric_scalar_too_long".into());
                }
            }
            _ => scalar = 0,
        }
    }
    Ok(())
}
pub fn decode_json(input: &[u8]) -> Result<Value, String> {
    decode_json_checked(input, &mut Unbounded).map_err(CheckedError::into_message)
}
pub(crate) fn decode_json_checked<C: Checkpoint>(
    input: &[u8],
    control: &mut C,
) -> Result<Value, CheckedError<C::Error>> {
    check_json_limits_checked(input, control)?;
    parse_json_checked(input, control)
}
fn parse_json_checked<C: Checkpoint>(
    input: &[u8],
    control: &mut C,
) -> Result<Value, CheckedError<C::Error>> {
    // Serde's default guard rejects our inclusive depth-128 boundary. The
    // mandatory lexical guard above enforces that limit before parsing; disable
    // only Serde's extra guard here, never the application's byte/depth caps.
    // Serde's reader path calls Read throughout strings, arrays, and objects.
    // Buffer only a bounded amount so a long scalar or unknown subtree cannot
    // hide the rest of a response from the processing control. Serde remains
    // the syntax/number parser; this adapter performs no JSON interpretation.
    // Pure/fuzz entry points use this same parser with an infallible control.
    let mut reader = CheckedReader {
        input,
        control,
        stopped: None,
    };
    let result = {
        let buffer = BufReader::with_capacity(DECODE_CHUNK, &mut reader);
        let mut decoder = serde_json::Deserializer::from_reader(buffer);
        decoder.disable_recursion_limit();
        Value::deserialize(&mut decoder).and_then(|value| decoder.end().map(|()| value))
    };
    if let Some(reason) = reader.stopped {
        return Err(CheckedError::Interrupted(reason));
    }
    result.map_err(|_| CheckedError::Invalid("invalid_json".into()))
}

const DECODE_CHUNK: usize = work::BYTE_CHUNK;
struct CheckedReader<'a, 'c, C: Checkpoint> {
    input: &'a [u8],
    control: &'c mut C,
    stopped: Option<C::Error>,
}
impl<C: Checkpoint> Read for CheckedReader<'_, '_, C> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.stopped.is_some() {
            return Err(io::Error::other("JSON processing interrupted"));
        }
        if let Err(reason) = self.control.boundary() {
            self.stopped = Some(reason);
            // Interrupted would be retried by Read consumers, hiding shutdown.
            return Err(io::Error::other("JSON processing interrupted"));
        }
        let count = self.input.len().min(bytes.len()).min(DECODE_CHUNK);
        bytes[..count].copy_from_slice(&self.input[..count]);
        self.input = &self.input[count..];
        Ok(count)
    }
}

pub fn string_at<'a>(value: &'a Value, path: &str) -> Option<&'a str> {
    value.pointer(path).and_then(Value::as_str)
}
pub fn strings_at(value: &Value, path: &str) -> Vec<String> {
    value
        .pointer(path)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
#[path = "../tests/unit/transport.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/unit/transport_projection.rs"]
mod projection_tests;
