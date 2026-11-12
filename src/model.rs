//! Validated domain values. Serialization is owned by the report adapter.
use crate::{
    cli::{Kind, Severity},
    qty::{Bytes, ExactRatio, Inodes},
    work::{Checkpoint, Unbounded},
};
use jiff::Timestamp;
use std::{collections::BTreeSet, fmt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityError;
impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("identity must be nonempty without colon or control characters")
    }
}
impl std::error::Error for IdentityError {}

fn valid_uid(uid: &str) -> bool {
    valid_uid_checked(uid, &mut Unbounded).unwrap_or_else(|never| match never {})
}
pub(crate) fn valid_uid_checked<C: Checkpoint>(
    uid: &str,
    control: &mut C,
) -> Result<bool, C::Error> {
    if uid.is_empty() {
        return Ok(false);
    }
    for ch in uid.chars() {
        control.step()?;
        if ch == ':' || ch.is_control() {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordId(String);
impl RecordId {
    pub fn new(kind: Kind, uid: &str) -> Result<Self, IdentityError> {
        if !valid_uid(uid) {
            return Err(IdentityError);
        }
        Ok(Self(format!(
            "{}:{uid}",
            if kind == Kind::Pvc { "pvc" } else { "pv" }
        )))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConsumerId(String);
impl ConsumerId {
    pub fn new(uid: &str) -> Result<Self, IdentityError> {
        if !valid_uid(uid) {
            return Err(IdentityError);
        }
        Ok(Self(format!("pod:{uid}")))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataValue<T> {
    Known(T),
    Absent,
    Unassigned,
    Unknown,
}
impl<T> MetadataValue<T> {
    pub fn known(&self) -> Option<&T> {
        if let Self::Known(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub const fn state(&self) -> &'static str {
        match self {
            Self::Known(_) => "known",
            Self::Absent => "absent",
            Self::Unassigned => "unassigned",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field<T> {
    Valid(T),
    Missing,
    Invalid,
}
impl<T: Copy> Field<T> {
    pub fn value(&self) -> Option<T> {
        if let Self::Valid(value) = self {
            Some(*value)
        } else {
            None
        }
    }
    pub const fn status(&self) -> &'static str {
        match self {
            Self::Valid(_) => "ok",
            Self::Missing => "missing",
            Self::Invalid => "invalid",
        }
    }
}

pub trait Counter: Copy + Eq + fmt::Debug {
    const FIELDS: [&'static str; 3];
    fn from_raw(value: u64) -> Self;
    fn raw(self) -> u64;
}
impl Counter for Bytes {
    const FIELDS: [&'static str; 3] = ["capacity_bytes", "used_bytes", "available_bytes"];
    fn from_raw(value: u64) -> Self {
        Self::new(value)
    }
    fn raw(self) -> u64 {
        self.get()
    }
}
impl Counter for Inodes {
    const FIELDS: [&'static str; 3] = ["inodes_total", "inodes_used", "inodes_free"];
    fn from_raw(value: u64) -> Self {
        Self::new(value)
    }
    fn raw(self) -> u64 {
        self.get()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Pending,
    Bound,
    Lost,
    Available,
    Released,
    Failed,
    Other(String),
}
impl Phase {
    pub fn from_api_for(kind: Kind, value: &str) -> Self {
        let phase = Self::from_api(value);
        if matches!(
            (kind, &phase),
            (Kind::Pvc, Self::Pending | Self::Bound | Self::Lost)
                | (
                    Kind::Pv,
                    Self::Available | Self::Bound | Self::Released | Self::Failed
                )
        ) {
            phase
        } else {
            Self::Other(value.into())
        }
    }
    pub fn from_api(value: &str) -> Self {
        match value {
            "Pending" => Self::Pending,
            "Bound" => Self::Bound,
            "Lost" => Self::Lost,
            "Available" => Self::Available,
            "Released" => Self::Released,
            "Failed" => Self::Failed,
            _ => Self::Other(value.to_owned()),
        }
    }
    pub fn as_str(&self) -> &str {
        match self {
            Self::Pending => "Pending",
            Self::Bound => "Bound",
            Self::Lost => "Lost",
            Self::Available => "Available",
            Self::Released => "Released",
            Self::Failed => "Failed",
            Self::Other(s) => s,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VolumeMode {
    Filesystem,
    Block,
    Unsupported(String),
    Invalid,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceState {
    Active,
    Terminating,
    Terminal,
    Unknown,
}
impl ReferenceState {
    pub const fn live(self) -> bool {
        matches!(self, Self::Active | Self::Terminating)
    }
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Terminating => "terminating",
            Self::Terminal => "terminal",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Consumer {
    pub id: ConsumerId,
    pub uid: String,
    pub namespace: String,
    pub name: String,
    pub phase: String,
    pub deletion_timestamp: Option<Timestamp>,
    pub node: Option<String>,
    pub state: ReferenceState,
    pub statefulset: MetadataValue<StatefulSet>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatefulSet {
    pub name: String,
    pub uid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContainerKind {
    App,
    Init,
    Ephemeral,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MountKind {
    Filesystem,
    Block,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Mount {
    pub container_kind: ContainerKind,
    pub container: String,
    pub volume_name: String,
    pub path: String,
    pub kind: MountKind,
    pub read_only: Option<bool>,
    pub sub_path: Option<String>,
    pub sub_path_expr: Option<String>,
}
#[derive(Debug, Clone)]
pub struct Relationship {
    pub record_id: RecordId,
    pub consumer_id: ConsumerId,
    pub volume_names: BTreeSet<String>,
    pub mounts: Vec<Mount>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Source {
    pub node: String,
    pub pod_id: ConsumerId,
    pub volume_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eligibility {
    Fresh,
    Stale,
    TimestampUnknown,
    ClockSkew,
    InvalidTimestamp,
    IdentityUncertain,
}

#[derive(Debug, Clone)]
pub struct Candidate<T: Counter> {
    pub capacity: Field<T>,
    pub used: Field<T>,
    pub available: Field<T>,
    pub observed_at: Option<Timestamp>,
    pub source: Source,
    pub eligibility: Eligibility,
}
impl<T: Counter> Candidate<T> {
    pub fn reason(&self) -> Option<String> {
        match self.eligibility {
            Eligibility::Stale => Some("max_age_exceeded".into()),
            Eligibility::TimestampUnknown => Some("timestamp_unknown".into()),
            Eligibility::ClockSkew => Some("clock_skew".into()),
            Eligibility::InvalidTimestamp => Some("invalid_timestamp".into()),
            Eligibility::IdentityUncertain => Some("identity_uncertain".into()),
            Eligibility::Fresh => T::FIELDS
                .into_iter()
                .zip([&self.capacity, &self.used, &self.available])
                .find(|(_, field)| !matches!(field, Field::Valid(_)))
                .map(|(key, field)| format!("{key}_{}", field.status())),
        }
    }
    pub fn complete(&self) -> bool {
        [&self.capacity, &self.used, &self.available]
            .iter()
            .all(|f| matches!(f, Field::Valid(_)))
    }
    pub fn usable(&self) -> bool {
        [&self.capacity, &self.used, &self.available]
            .iter()
            .any(|f| matches!(f, Field::Valid(_)))
    }
    pub fn ratio(&self) -> Option<ExactRatio> {
        ExactRatio::new(
            self.used.value()?.raw() as u128,
            self.capacity.value()?.raw() as u128,
        )
        .ok()
    }
    pub fn status(&self) -> &'static str {
        match self.eligibility {
            Eligibility::Fresh if self.complete() => "ok",
            Eligibility::Fresh if self.usable() => "partial",
            Eligibility::Fresh => "invalid",
            Eligibility::Stale => "stale",
            Eligibility::TimestampUnknown => "timestamp_unknown",
            Eligibility::ClockSkew | Eligibility::InvalidTimestamp => "invalid",
            Eligibility::IdentityUncertain => "identity_uncertain",
        }
    }
}

#[derive(Debug, Clone)]
pub enum MetricFamily<T: Counter> {
    NotRequested,
    NotApplicable,
    Unavailable {
        status: String,
        reason: String,
    },
    Observed {
        selected: Candidate<T>,
        candidates: Vec<Candidate<T>>,
        disagreement: Option<(ExactRatio, ExactRatio)>,
    },
}
impl<T: Counter> MetricFamily<T> {
    /// Display range preserves repeated fresh observations even without a
    /// conflict. Conflict classification uses only complete, co-timed tuples.
    pub fn observed_range(&self) -> Option<(ExactRatio, ExactRatio)> {
        self.observed_range_checked(&mut Unbounded)
            .unwrap_or_else(|never| match never {})
    }
    pub(crate) fn observed_range_checked<C: Checkpoint>(
        &self,
        control: &mut C,
    ) -> Result<Option<(ExactRatio, ExactRatio)>, C::Error> {
        let Self::Observed {
            candidates,
            disagreement,
            ..
        } = self
        else {
            return Ok(None);
        };
        if disagreement.is_some() || candidates.len() < 2 {
            return Ok(*disagreement);
        }
        let mut range = None;
        for candidate in candidates {
            control.step()?;
            if candidate.eligibility == Eligibility::Fresh
                && let Some(ratio) = candidate.ratio()
            {
                range = Some(match range {
                    Some((min, max)) => (ratio.min(min), ratio.max(max)),
                    None => (ratio, ratio),
                });
            }
        }
        Ok(range)
    }
    pub fn reason(&self) -> Option<String> {
        match self {
            Self::Unavailable { reason, .. } => Some(reason.clone()),
            Self::Observed {
                disagreement: Some(_),
                ..
            } => Some("metrics_disagreement".into()),
            Self::Observed { selected, .. } => selected.reason(),
            _ => None,
        }
    }
    pub fn selected(&self) -> Option<&Candidate<T>> {
        if let Self::Observed { selected, .. } = self {
            Some(selected)
        } else {
            None
        }
    }
    pub fn fresh_unambiguous(&self) -> Option<&Candidate<T>> {
        match self {
            Self::Observed {
                selected,
                disagreement: None,
                ..
            } if selected.eligibility == Eligibility::Fresh => Some(selected),
            _ => None,
        }
    }
    pub fn status(&self) -> &str {
        match self {
            Self::NotRequested => "not_requested",
            Self::NotApplicable => "not_applicable",
            Self::Unavailable { status, .. } => status,
            Self::Observed {
                disagreement: Some(_),
                ..
            } => "uncertain",
            Self::Observed { selected, .. } => selected.status(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ConsumerVisibility {
    Complete,
    Unknown(String),
}
#[derive(Debug, Clone)]
pub struct PodOwner {
    pub name: String,
    pub id: Option<ConsumerId>,
}
#[derive(Debug, Clone)]
pub struct Record {
    pub id: RecordId,
    pub kind: Kind,
    pub uid: String,
    pub namespace: MetadataValue<String>,
    pub name: String,
    pub created_at: Option<Timestamp>,
    pub deletion_timestamp: Option<Timestamp>,
    pub pod_owner: Option<PodOwner>,
    pub claim_created_at: Option<Timestamp>,
    pub phase: Phase,
    pub storage_class: MetadataValue<String>,
    pub volume_binding_mode: Option<String>,
    pub driver: MetadataValue<String>,
    pub volume_mode: VolumeMode,
    pub access_modes: Vec<String>,
    pub reclaim_policy: Option<String>,
    pub pv_name: Option<String>,
    pub pvc_name: MetadataValue<String>,
    pub pvc_uid: Option<String>,
    pub requested: Field<Bytes>,
    pub bound: Field<Bytes>,
    pub provisioned: Field<Bytes>,
    pub consumers: BTreeSet<ConsumerId>,
    pub consumer_visibility: ConsumerVisibility,
    pub active_count: Option<usize>,
    pub terminating_count: Option<usize>,
    pub binding_conflict: bool,
    pub filesystem: MetricFamily<Bytes>,
    pub inodes: MetricFamily<Inodes>,
    pub overlapping_filesystem: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    Match,
    NoMatch,
    Unknown(BTreeSet<String>),
}
impl Predicate {
    pub fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::NoMatch, _) | (_, Self::NoMatch) => Self::NoMatch,
            (Self::Match, value) | (value, Self::Match) => value,
            (Self::Unknown(mut a), Self::Unknown(b)) => {
                a.extend(b);
                Self::Unknown(a)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub code: String,
    pub subject_id: Option<RecordId>,
    pub source: String,
    pub reason: String,
    pub affected_fields: Vec<String>,
    pub message: String,
}
#[derive(Debug, Clone)]
pub struct Evidence {
    pub field: String,
    pub value: serde_json::Value,
    pub unit: Option<String>,
    pub observed_at: Option<Timestamp>,
    pub source: Option<Source>,
}
#[derive(Debug, Clone)]
pub struct Finding {
    pub code: String,
    pub severity: Severity,
    pub subject_id: RecordId,
    pub evidence: Vec<Evidence>,
    pub suggested_action: String,
}

#[cfg(test)]
#[path = "../tests/unit/model.rs"]
mod tests;
