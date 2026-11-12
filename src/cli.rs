//! Validate the complete P0 invocation before constructing a network client.
use crate::qty::Threshold;
use clap::{Parser, ValueEnum};
use serde::Serialize;
use std::{fmt, path::PathBuf, str::FromStr, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Pvc,
    Pv,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum View {
    Tree,
    Table,
    Summary,
    Issues,
    Stats,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Output {
    Text,
    Json,
    Csv,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Color {
    Auto,
    Always,
    Never,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Dimension {
    Storageclass,
    Namespace,
    Pvc,
    Pv,
    Driver,
    Node,
    Pod,
    Accessmode,
    Volumemode,
    Statefulset,
    Phase,
}
impl FromStr for Dimension {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "storageclass" => Ok(Self::Storageclass),
            "namespace" => Ok(Self::Namespace),
            "pvc" => Ok(Self::Pvc),
            "pv" => Ok(Self::Pv),
            "driver" => Ok(Self::Driver),
            "node" => Ok(Self::Node),
            "pod" => Ok(Self::Pod),
            "accessmode" | "access-mode" => Ok(Self::Accessmode),
            "volumemode" | "volume-mode" => Ok(Self::Volumemode),
            "statefulset" | "sts" => Ok(Self::Statefulset),
            "phase" => Ok(Self::Phase),
            _ => Err(format!("unsupported grouping dimension: {s}")),
        }
    }
}
impl fmt::Display for Dimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Storageclass => "storageclass",
            Self::Namespace => "namespace",
            Self::Pvc => "pvc",
            Self::Pv => "pv",
            Self::Driver => "driver",
            Self::Node => "node",
            Self::Pod => "pod",
            Self::Accessmode => "accessmode",
            Self::Volumemode => "volumemode",
            Self::Statefulset => "statefulset",
            Self::Phase => "phase",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Name,
    Requested,
    Provisioned,
    Used,
    Available,
    UsedPercent,
    InodePercent,
    Severity,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort {
    pub field: SortField,
    pub descending: bool,
}
impl FromStr for Sort {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (name, direction) = s.split_once(':').unwrap_or((s, "asc"));
        let field = match name {
            "name" => SortField::Name,
            "requested" => SortField::Requested,
            "provisioned" => SortField::Provisioned,
            "used" => SortField::Used,
            "available" => SortField::Available,
            "used-percent" => SortField::UsedPercent,
            "inode-percent" => SortField::InodePercent,
            "severity" => SortField::Severity,
            _ => return Err(format!("unsupported sort field: {name}")),
        };
        let descending = match direction {
            "asc" => false,
            "desc" => true,
            _ => return Err("sort direction must be asc or desc".into()),
        };
        Ok(Self { field, descending })
    }
}
impl fmt::Display for Sort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.field {
            SortField::Name => "name",
            SortField::Requested => "requested",
            SortField::Provisioned => "provisioned",
            SortField::Used => "used",
            SortField::Available => "available",
            SortField::UsedPercent => "used-percent",
            SortField::InodePercent => "inode-percent",
            SortField::Severity => "severity",
        };
        write!(f, "{name}:{}", if self.descending { "desc" } else { "asc" })
    }
}

pub fn duration(s: &str) -> Result<Duration, String> {
    let split = s
        .find(|c: char| !c.is_ascii_digit())
        .ok_or("duration requires ms, s, m, or h")?;
    let number = s[..split]
        .parse::<u64>()
        .map_err(|_| "duration requires a positive integer")?;
    let multiplier = match &s[split..] {
        "ms" => 1,
        "s" => 1000,
        "m" => 60000,
        "h" => 3600000,
        _ => return Err("duration unit must be ms, s, m, or h".into()),
    };
    let millis = number
        .checked_mul(multiplier)
        .filter(|n| *n > 0)
        .ok_or("duration is zero or out of range")?;
    let value = Duration::from_millis(millis);
    if std::time::Instant::now().checked_add(value).is_none() {
        return Err("duration exceeds monotonic clock range".into());
    }
    Ok(value)
}

#[derive(Debug, Clone, Serialize)]
pub struct Filters {
    pub selector: Option<String>,
    pub pvc: Option<String>,
    pub pv: Option<String>,
    pub storage_class: Option<String>,
    pub driver: Option<String>,
    pub node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statefulset: Option<String>,
}

#[derive(Debug, Clone, Parser)]
#[command(
    name = "kubectl-view-storage",
    version,
    about = "Inspect persistent storage inventory, consumers, and usage"
)]
pub struct Cli {
    #[arg(short = 'n', long, conflicts_with = "all_namespaces")]
    pub namespace: Option<String>,
    #[arg(short = 'A', long)]
    pub all_namespaces: bool,
    #[arg(long)]
    pub context: Option<String>,
    #[arg(long)]
    pub kubeconfig: Option<PathBuf>,
    #[arg(short = 'u', long)]
    pub utilization: bool,
    #[arg(short = 'g', long = "group-by", value_delimiter = ',')]
    pub group_by: Option<Vec<Dimension>>,
    #[arg(long, value_enum, default_value = "tree")]
    pub view: View,
    #[arg(long)]
    pub wide: bool,
    #[arg(short = 'o', long, value_enum, default_value = "text")]
    pub output: Output,
    #[arg(long, value_enum, default_value = "pvc")]
    pub kind: Kind,
    #[arg(short = 'l', long)]
    pub selector: Option<String>,
    #[arg(long)]
    pub pvc: Option<String>,
    #[arg(long)]
    pub pv: Option<String>,
    #[arg(long)]
    pub storage_class: Option<String>,
    #[arg(long)]
    pub driver: Option<String>,
    #[arg(long)]
    pub node: Option<String>,
    /// Select claims referenced by live pods controlled by this StatefulSet.
    #[arg(long, alias = "sts")]
    pub statefulset: Option<String>,
    /// Units for the stats view: auto, B, KB..PB, or KiB..PiB (case insensitive).
    #[arg(long, alias = "unit", default_value = "auto")]
    pub units: crate::calculations::Unit,
    /// Earlier JSON report used to calculate observed growth and time to full.
    #[arg(long, value_name = "REPORT.json")]
    pub compare: Option<PathBuf>,
    #[arg(long, default_value = "name:asc")]
    pub sort: Sort,
    #[arg(long, value_parser=duration, default_value="10s")]
    pub request_timeout: Duration,
    #[arg(long, value_parser=duration, default_value="60s")]
    pub timeout: Duration,
    #[arg(long, value_parser=clap::value_parser!(u8).range(1..=64), default_value_t=8)]
    pub concurrency: u8,
    #[arg(long, value_parser=duration, default_value="5m")]
    pub max_metrics_age: Duration,
    #[arg(long, default_value = "80")]
    pub warn_used: Threshold,
    #[arg(long, default_value = "90")]
    pub critical_used: Threshold,
    #[arg(long)]
    pub require_metrics: bool,
    #[arg(long, value_parser=fail_level)]
    pub fail_on: Option<Severity>,
    #[arg(long, value_enum, default_value = "auto")]
    pub color: Color,
}

#[derive(Debug, Clone)]
pub struct Query {
    pub kind: Kind,
    pub namespace: Option<String>,
    pub group_by: Vec<Dimension>,
    pub filters: Filters,
    pub utilization: bool,
    pub max_metrics_age: Duration,
    pub warn_used: Threshold,
    pub critical_used: Threshold,
    pub require_metrics: bool,
    pub fail_on: Option<Severity>,
    pub sort: Sort,
    pub statistics: bool,
    pub units: crate::calculations::Unit,
    pub baseline: Option<std::sync::Arc<crate::calculations::Baseline>>,
}

fn fail_level(value: &str) -> Result<Severity, String> {
    match value {
        "warning" => Ok(Severity::Warning),
        "critical" => Ok(Severity::Critical),
        _ => Err("failure level must be warning or critical".into()),
    }
}

impl Cli {
    pub fn validate(&self) -> Result<(), String> {
        if self.view != View::Stats
            && (self.compare.is_some() || self.units != crate::calculations::Unit::Auto)
        {
            return Err("--units and --compare require --view stats".into());
        }
        if self.all_namespaces && self.statefulset.is_some() {
            return Err("--statefulset requires a single namespace; use -n NAMESPACE".into());
        }
        if self.kind == Kind::Pv && self.statefulset.is_some() {
            return Err("--statefulset requires PVC mode".into());
        }
        if self.kind == Kind::Pv
            && (!self.all_namespaces || self.namespace.is_some() || self.pvc.is_some())
        {
            return Err("--kind pv requires -A and rejects --namespace and --pvc".into());
        }
        if self.warn_used >= self.critical_used {
            return Err("thresholds must satisfy 0 < warning < critical <= 100".into());
        }
        if let Some(dimensions) = &self.group_by
            && (dimensions.is_empty()
                || dimensions
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != dimensions.len())
        {
            return Err("grouping requires at least one dimension and no duplicates".into());
        }
        for (flag, value) in [
            ("namespace", &self.namespace),
            ("context", &self.context),
            ("pvc", &self.pvc),
            ("pv", &self.pv),
            ("driver", &self.driver),
            ("node", &self.node),
            ("statefulset", &self.statefulset),
        ] {
            if value
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.chars().any(char::is_control))
            {
                return Err(format!(
                    "--{flag} must be nonempty without control characters"
                ));
            }
        }
        Ok(())
    }
    pub fn query(&self, default_namespace: &str) -> Result<Query, String> {
        self.validate()?;
        Ok(Query {
            kind: self.kind,
            namespace: if self.all_namespaces {
                None
            } else {
                Some(
                    self.namespace
                        .as_deref()
                        .unwrap_or(default_namespace)
                        .to_owned(),
                )
            },
            group_by: self.group_by.clone().unwrap_or_else(|| {
                if self.view == View::Stats {
                    vec![Dimension::Storageclass]
                } else if self.kind == Kind::Pv {
                    vec![Dimension::Storageclass, Dimension::Pv]
                } else {
                    vec![
                        Dimension::Storageclass,
                        Dimension::Namespace,
                        Dimension::Pvc,
                    ]
                }
            }),
            filters: Filters {
                selector: self.selector.clone(),
                pvc: self.pvc.clone(),
                pv: self.pv.clone(),
                storage_class: self.storage_class.clone(),
                driver: self.driver.clone(),
                node: self.node.clone(),
                statefulset: self.statefulset.clone(),
            },
            utilization: self.utilization || self.require_metrics || self.compare.is_some(),
            max_metrics_age: self.max_metrics_age,
            warn_used: self.warn_used,
            critical_used: self.critical_used,
            require_metrics: self.require_metrics,
            fail_on: self.fail_on,
            sort: self.sort,
            statistics: self.view == View::Stats,
            units: self.units,
            baseline: None,
        })
    }
}

#[cfg(test)]
#[path = "../tests/unit/cli.rs"]
mod tests;
