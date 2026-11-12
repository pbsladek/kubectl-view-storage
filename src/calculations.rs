//! Opt-in operator calculations over the same selected, UID-deduplicated report.
//! All arithmetic stays integral; rounding is confined to terminal/CSV display.
use crate::{
    cli::Query,
    work::{self, CheckedError, Checkpoint},
};
use jiff::Timestamp;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fmt, fs::OpenOptions, io::Read, path::Path, str::FromStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Auto,
    B,
    Kb,
    Mb,
    Gb,
    Tb,
    Pb,
    Kib,
    Mib,
    Gib,
    Tib,
    Pib,
}
impl Unit {
    const ALL: [Self; 12] = [
        Self::Auto,
        Self::B,
        Self::Kb,
        Self::Mb,
        Self::Gb,
        Self::Tb,
        Self::Pb,
        Self::Kib,
        Self::Mib,
        Self::Gib,
        Self::Tib,
        Self::Pib,
    ];
    fn scale(self) -> u128 {
        match self {
            Self::Auto | Self::B => 1,
            Self::Kb => 1000,
            Self::Mb => 1000u128.pow(2),
            Self::Gb => 1000u128.pow(3),
            Self::Tb => 1000u128.pow(4),
            Self::Pb => 1000u128.pow(5),
            Self::Kib => 1024,
            Self::Mib => 1024u128.pow(2),
            Self::Gib => 1024u128.pow(3),
            Self::Tib => 1024u128.pow(4),
            Self::Pib => 1024u128.pow(5),
        }
    }
    pub fn format(self, numerator: u128, denominator: u128) -> String {
        if denominator == 0 {
            return "--".into();
        }
        let mut unit = self;
        if self == Self::Auto {
            unit = Self::B;
            for candidate in [Self::Kib, Self::Mib, Self::Gib, Self::Tib, Self::Pib] {
                if numerator / denominator >= candidate.scale() {
                    unit = candidate;
                }
            }
        }
        denominator
            .checked_mul(unit.scale())
            .and_then(|d| {
                let ordinary = decimal(numerator, d)?;
                if numerator != 0 && ordinary == "0.00" {
                    let precise = decimal_places(numerator, d, 6)?;
                    Some(if precise == "0.000000" {
                        "<0.000001".into()
                    } else {
                        precise
                    })
                } else {
                    Some(ordinary)
                }
            })
            .map(|amount| format!("{amount} {unit}"))
            .unwrap_or_else(|| "--".into())
    }
}
impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::B => "B",
            Self::Kb => "KB",
            Self::Mb => "MB",
            Self::Gb => "GB",
            Self::Tb => "TB",
            Self::Pb => "PB",
            Self::Kib => "KiB",
            Self::Mib => "MiB",
            Self::Gib => "GiB",
            Self::Tib => "TiB",
            Self::Pib => "PiB",
        })
    }
}
impl FromStr for Unit {
    type Err = String;
    fn from_str(input: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|unit| unit.to_string().eq_ignore_ascii_case(input))
            .ok_or_else(|| {
                "units must be auto, B, KB, MB, GB, TB, PB, KiB, MiB, GiB, TiB, or PiB".into()
            })
    }
}

fn decimal(n: u128, d: u128) -> Option<String> {
    decimal_places(n, d, 2)
}
fn decimal_places(n: u128, d: u128, places: u32) -> Option<String> {
    if d == 0 {
        return None;
    }
    let whole = n / d;
    let mut remainder = n % d;
    let scale = 10u128.pow(places);
    let mut digits = 0u128;
    for _ in 0..places {
        let (digit, rest) = crate::qty::decimal_step(remainder, d);
        digits = digits * 10 + u128::from(digit);
        remainder = rest;
    }
    let rounded = digits + u128::from(remainder >= d / 2 + d % 2);
    Some(format!(
        "{}.{:0width$}",
        whole.checked_add(rounded / scale)?,
        rounded % scale,
        width = places as usize,
    ))
}

#[derive(Debug, Clone)]
pub struct Baseline {
    document: Value,
}
impl Baseline {
    pub const MAX_BYTES: u64 = 32 * 1024 * 1024;
    pub fn load(path: &Path) -> Result<Self, String> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Do not block opening a FIFO before rejecting non-regular input.
            options.custom_flags(nix::libc::O_NONBLOCK);
        }
        let file = options
            .open(path)
            .map_err(|_| "cannot open comparison report")?;
        let metadata = file
            .metadata()
            .map_err(|_| "cannot inspect comparison report")?;
        if !metadata.is_file() || metadata.len() > Self::MAX_BYTES {
            return Err("comparison report must be a regular file of at most 32 MiB".into());
        }
        let mut bytes = vec![];
        file.take(Self::MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read comparison report")?;
        if bytes.len() as u64 > Self::MAX_BYTES {
            return Err("comparison report exceeds 32 MiB".into());
        }
        let value = serde_json::from_slice(&bytes)
            .map_err(|_| "comparison report is not valid bounded-depth JSON")?;
        Self::from_document(value)
    }
    pub fn from_document(document: Value) -> Result<Self, String> {
        if document["schema_version"] != "view-storage/v1alpha1"
            || document["query"]["utilization"] != true
            || !matches!(document["query"]["kind"].as_str(), Some("pvc" | "pv"))
            || !document["collection"]["context"].is_string()
            || !document["collection"]["scope"].is_object()
            || !document["query"]["filters"].is_object()
            || !document["selection"]["complete"].is_boolean()
            || timestamp(&document["collection"]["finished_at"]).is_none()
        {
            return Err(
                "comparison requires a view-storage/v1alpha1 JSON report collected with -u".into(),
            );
        }
        let records = document["records"]
            .as_array()
            .ok_or("comparison report has no record array")?;
        let mut ids = std::collections::BTreeSet::new();
        for record in records {
            let id = record["id"]
                .as_str()
                .ok_or("comparison record has no identity")?;
            let uid = record["uid"]
                .as_str()
                .ok_or("comparison record has no UID")?;
            let kind = document["query"]["kind"]
                .as_str()
                .ok_or("comparison has no kind")?;
            if id != format!("{kind}:{uid}")
                || uid.is_empty()
                || !ids.insert(id)
                || !record["filesystem"].is_object()
            {
                return Err(
                    "comparison report contains duplicate or invalid record identities".into(),
                );
            }
        }
        Ok(Self { document })
    }
    fn validate_scope(&self, current: &Value) -> Result<(), String> {
        let old = &self.document;
        for pointer in ["/query/kind", "/collection/context", "/collection/scope"] {
            if old.pointer(pointer) != current.pointer(pointer) {
                return Err(
                    "comparison requires the same context, scope and inventory kind".into(),
                );
            }
        }
        for key in [
            "selector",
            "pvc",
            "pv",
            "storage_class",
            "driver",
            "node",
            "statefulset",
        ] {
            if old["query"]["filters"][key] != current["query"]["filters"][key] {
                return Err("comparison requires the same selection filters".into());
            }
        }
        if timestamp(&old["collection"]["finished_at"])
            >= timestamp(&current["collection"]["finished_at"])
        {
            return Err("comparison report must precede the current collection".into());
        }
        Ok(())
    }
}

/// Reduced unsigned magnitude plus sign avoids floating point and i64 counters.
#[derive(Debug, Clone, Copy)]
struct Fraction {
    negative: bool,
    n: u128,
    d: u128,
}
fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
impl Fraction {
    fn new(negative: bool, n: u128, d: u128) -> Option<Self> {
        if d == 0 {
            return None;
        }
        let common = gcd(n, d);
        Some(Self {
            negative: negative && n != 0,
            n: n / common,
            d: d / common,
        })
    }
    fn add(self, other: Self) -> Option<Self> {
        let common = gcd(self.d, other.d);
        let left = self.n.checked_mul(other.d / common)?;
        let right = other.n.checked_mul(self.d / common)?;
        let denominator = self.d.checked_mul(other.d / common)?;
        if self.negative == other.negative {
            Self::new(self.negative, left.checked_add(right)?, denominator)
        } else if left >= right {
            Self::new(self.negative, left - right, denominator)
        } else {
            Self::new(other.negative, right - left, denominator)
        }
    }
    fn value(self) -> Value {
        json!({"numerator":format!("{}{}", if self.negative {"-"} else {""}, self.n),"denominator":self.d.to_string()})
    }
    fn forecast(self, available: u128) -> Option<Self> {
        if available == 0 {
            return Self::new(false, 0, 1);
        }
        if self.negative || self.n == 0 {
            return None;
        }
        let common = gcd(available, self.n);
        Self::new(
            false,
            (available / common).checked_mul(self.d)?,
            self.n / common,
        )
    }
}
fn number(value: &Value) -> Option<u128> {
    value.as_str()?.parse().ok()
}
fn timestamp(value: &Value) -> Option<Timestamp> {
    value.as_str()?.parse().ok()
}
fn fraction(value: &Value) -> Option<Fraction> {
    let n = value["numerator"].as_str()?;
    Fraction::new(
        n.starts_with('-'),
        n.trim_start_matches('-').parse().ok()?,
        number(&value["denominator"])?,
    )
}
pub fn format_fraction(value: &Value, unit: Unit) -> String {
    fraction(value)
        .map(|v| signed_amount(v.negative, unit.format(v.n, v.d)))
        .unwrap_or_else(|| "--".into())
}
pub fn format_hourly_rate(value: &Value, unit: Unit) -> String {
    fraction(value)
        .and_then(|v| v.n.checked_mul(3600).map(|n| (v, n)))
        .map(|(v, n)| signed_amount(v.negative, unit.format(n, v.d)))
        .unwrap_or_else(|| "--".into())
}
fn signed_amount(negative: bool, amount: String) -> String {
    if negative && amount.starts_with('<') {
        format!("decrease {amount}")
    } else {
        format!("{}{amount}", if negative { "-" } else { "" })
    }
}
pub fn format_seconds(value: &Value) -> String {
    let Some(value) = fraction(value) else {
        return "--".into();
    };
    let seconds = value.n / value.d;
    let (scale, unit) = if seconds >= 86400 {
        (86400, "d")
    } else if seconds >= 3600 {
        (3600, "h")
    } else if seconds >= 60 {
        (60, "m")
    } else {
        (1, "s")
    };
    value
        .d
        .checked_mul(scale)
        .and_then(|d| decimal(value.n, d))
        .map(|v| format!("{v} {unit}"))
        .unwrap_or_else(|| "--".into())
}

fn distribution<C: Checkpoint>(
    mut values: Vec<u128>,
    total: usize,
    complete: bool,
    control: &mut C,
) -> Result<Value, C::Error> {
    work::sort_by(&mut values, Ord::cmp, control)?;
    let mut sum = Some(0u128);
    for &value in &values {
        control.step()?;
        sum = sum.and_then(|sum| sum.checked_add(value));
    }
    if values.is_empty() && total != 0 {
        sum = None;
    }
    let n = values.len();
    let median = if n == 0 {
        None
    } else if n % 2 == 1 {
        Fraction::new(false, values[n / 2], 1)
    } else {
        values[n / 2 - 1]
            .checked_add(values[n / 2])
            .and_then(|n| Fraction::new(false, n, 2))
    };
    Ok(
        json!({"known_count":n,"total_count":total,"complete":complete && n==total && sum.is_some(),
        "sum_bytes":sum.map(|v|v.to_string()),"min_bytes":values.first().map(u128::to_string),"max_bytes":values.last().map(u128::to_string),
        "mean_bytes":sum.and_then(|sum|Fraction::new(false,sum,n as u128)).map(Fraction::value),
        "median_bytes":median.map(Fraction::value),"p95_bytes":if n==0 {None} else {Some(values[n - n/20 - 1].to_string())}}),
    )
}

struct Rate {
    value: Value,
    rate: Option<Fraction>,
    full: Option<Fraction>,
}
fn sample(record: &Value) -> Option<(u128, u128, u128, Timestamp)> {
    let fs = &record["filesystem"];
    if fs["status"] != "ok"
        || fs["source"]["provider"] != "kubelet"
        || record["overlapping_filesystem"] == true
        || record["volume_mode"] != "Filesystem"
    {
        return None;
    }
    let (capacity, used, available) = (
        number(&fs["capacity_bytes"])?,
        number(&fs["used_bytes"])?,
        number(&fs["available_bytes"])?,
    );
    if capacity == 0 || capacity > u64::MAX as u128 || used.checked_add(available)? > capacity {
        return None;
    }
    Some((capacity, used, available, timestamp(&fs["observed_at"])?))
}
fn rate(record: &Value, previous: Option<&&Value>) -> Rate {
    if record["volume_mode"] == "Block" {
        return Rate {
            value: json!({"record_id":record["id"],"status":"not_applicable","reason":"raw_block","growth_bytes_per_second":null,"time_to_full_seconds":null}),
            rate: None,
            full: None,
        };
    }
    let failure = |reason| Rate {
        value: json!({"record_id":record["id"],"status":"unavailable","reason":reason,"growth_bytes_per_second":null,"time_to_full_seconds":null}),
        rate: None,
        full: None,
    };
    let Some(previous) = previous else {
        return failure("no_previous_identity");
    };
    for key in [
        "uid",
        "kind",
        "namespace",
        "name",
        "created_at",
        "pv_name",
        "pvc_name",
        "pvc_uid",
        "storage_class",
        "storage_class_state",
        "driver",
        "volume_mode",
    ] {
        if record[key] != previous[key] {
            return failure("identity_or_binding_changed");
        }
    }
    // A PV can outlive its claim, including a replacement with the same name.
    // Older reports lack this evidence and cannot establish a stable binding.
    if record["kind"] == "pv"
        && [record, *previous].iter().any(|record| {
            ["pvc_name", "pvc_uid"]
                .iter()
                .any(|key| record[key].as_str().is_none_or(|value| value.is_empty()))
        })
    {
        return failure("incomplete_or_unusable_samples");
    }
    let (Some((capacity, used, available, time)), Some((old_capacity, old_used, _, old_time))) =
        (sample(record), sample(previous))
    else {
        return failure("incomplete_or_unusable_samples");
    };
    if capacity != old_capacity {
        return failure("capacity_changed");
    }
    let elapsed = time.as_nanosecond() - old_time.as_nanosecond();
    if elapsed <= 0 {
        return failure("sample_not_newer");
    }
    let Some(rate) = used
        .abs_diff(old_used)
        .checked_mul(1_000_000_000)
        .and_then(|n| Fraction::new(used < old_used, n, elapsed as u128))
    else {
        return failure("arithmetic_overflow");
    };
    let full = rate.forecast(available);
    let reason = (!rate.negative && rate.n > 0 && full.is_none()).then_some("projection_overflow");
    let status = if rate.n == 0 {
        "steady"
    } else if rate.negative {
        "shrinking"
    } else {
        "growing"
    };
    Rate {
        value: json!({"record_id":record["id"],"status":status,"reason":reason,"interval_nanoseconds":elapsed.to_string(),"growth_bytes_per_second":rate.value(),"time_to_full_seconds":full.map(Fraction::value)}),
        rate: Some(rate),
        full,
    }
}

fn growth<C: Checkpoint>(
    records: &[&Value],
    rates: &BTreeMap<&str, Rate>,
    selection_complete: bool,
    control: &mut C,
) -> Result<Value, C::Error> {
    let (mut eligible, mut matched) = (0, 0);
    let mut sum = Fraction::new(false, 0, 1);
    let mut first: Option<Fraction> = None;
    let mut projection_overflow = false;
    for record in records {
        control.step()?;
        if record["volume_mode"] == "Block" {
            continue;
        }
        eligible += 1;
        if let Some(rate) = record["id"].as_str().and_then(|id| rates.get(id)) {
            projection_overflow |= rate.value["reason"] == "projection_overflow";
            if let Some(value) = rate.rate {
                matched += 1;
                sum = sum.and_then(|sum| sum.add(value));
            }
            if let Some(value) = rate.full {
                // Fraction comparison uses the overflow-safe continued-fraction implementation.
                if first.is_none_or(|old| {
                    crate::qty::ExactRatio::new(value.n, value.d).ok()
                        < crate::qty::ExactRatio::new(old.n, old.d).ok()
                }) {
                    first = Some(value);
                }
            }
        }
    }
    Ok(
        json!({"eligible_count":eligible,"matched_count":matched,"complete":selection_complete && matched==eligible && sum.is_some() && !projection_overflow,
        "growth_bytes_per_second":if matched==0 {None} else {sum.map(Fraction::value)},"first_full_seconds":first.map(Fraction::value),
        "reason":if sum.is_none() || projection_overflow {Some("arithmetic_overflow")} else if matched<eligible {Some("incomplete_pairs")} else {None}}),
    )
}

fn row<C: Checkpoint>(
    id: &str,
    label: &str,
    records: &[&Value],
    totals: &Value,
    rates: Option<&BTreeMap<&str, Rate>>,
    control: &mut C,
) -> Result<Value, C::Error> {
    let complete = totals["selection_complete"] == true;
    let mut stats = serde_json::Map::new();
    for key in ["requested", "bound", "provisioned"] {
        let mut values = vec![];
        for record in records {
            control.step()?;
            if let Some(value) = number(&record[format!("{key}_bytes")]) {
                values.push(value);
            }
        }
        stats.insert(
            key.into(),
            distribution(values, records.len(), complete, control)?,
        );
    }
    let (mut pairs, mut expansion, mut excess) = (0, Some(0u128), Some(0u128));
    for record in records {
        control.step()?;
        if let (Some(requested), Some(bound), Some(provisioned)) = (
            number(&record["requested_bytes"]),
            number(&record["bound_bytes"]),
            number(&record["provisioned_bytes"]),
        ) {
            pairs += 1;
            expansion = expansion.and_then(|sum| sum.checked_add(requested.saturating_sub(bound)));
            excess = excess.and_then(|sum| sum.checked_add(provisioned.saturating_sub(requested)));
        }
    }
    let mut row = json!({"id":id,"label":label,"statistics":stats,"capacity_gaps":{"known_count":pairs,"total_count":records.len(),"complete":complete && pairs==records.len() && expansion.is_some() && excess.is_some(),
        "requested_above_bound_bytes":if pairs==0 && !records.is_empty() {None} else {expansion.map(|v|v.to_string())},"provisioned_above_requested_bytes":if pairs==0 && !records.is_empty() {None} else {excess.map(|v|v.to_string())}}});
    row["totals"] = work::clone_json(totals, control)?;
    if let Some(rates) = rates {
        row["growth"] = growth(records, rates, complete, control)?;
    }
    Ok(row)
}
fn groups<C: Checkpoint>(
    input: &Value,
    records: &BTreeMap<&str, &Value>,
    rates: Option<&BTreeMap<&str, Rate>>,
    parent: &str,
    rows: &mut Vec<Value>,
    control: &mut C,
) -> Result<(), C::Error> {
    for group in input.as_array().into_iter().flatten() {
        control.step()?;
        let raw = group["value"]
            .as_str()
            .unwrap_or_else(|| match group["value_state"].as_str() {
                Some("absent") if group["dimension"] == "storageclass" => "(classless)",
                Some("absent") => "(none)",
                Some("unassigned") => "(unassigned)",
                _ => "(unknown)",
            });
        let value = if group["dimension"] == "statefulset" || group["dimension"] == "pod" {
            raw.rsplit_once('@').map(|(name, _)| name).unwrap_or(raw)
        } else {
            raw
        };
        let label = format!(
            "{}{}{}",
            parent,
            if parent.is_empty() { "" } else { " / " },
            value,
        );
        let mut members = vec![];
        for id in group["member_ids"].as_array().into_iter().flatten() {
            control.step()?;
            if let Some(record) = id.as_str().and_then(|id| records.get(id)) {
                members.push(*record);
            }
        }
        rows.push(row(
            group["id"].as_str().unwrap_or(""),
            &label,
            &members,
            &group["totals"],
            rates,
            control,
        )?);
        groups(&group["children"], records, rates, &label, rows, control)?;
    }
    Ok(())
}
pub(crate) fn attach_checked<C: Checkpoint>(
    report: &mut Value,
    query: &Query,
    control: &mut C,
) -> Result<(), CheckedError<C::Error>> {
    let mut records = BTreeMap::new();
    for record in report["records"].as_array().into_iter().flatten() {
        control.checked()?;
        if let Some(id) = record["id"].as_str() {
            records.insert(id, record);
        }
    }
    let mut rates = BTreeMap::new();
    let mut comparison = Value::Null;
    if let Some(baseline) = &query.baseline {
        baseline.validate_scope(report)?;
        let mut old = BTreeMap::new();
        for record in baseline.document["records"]
            .as_array()
            .into_iter()
            .flatten()
        {
            control.checked()?;
            if let Some(id) = record["id"].as_str() {
                old.insert(id, record);
            }
        }
        for (&id, &record) in &records {
            control.checked()?;
            rates.insert(id, rate(record, old.get(id)));
        }
        let mut entries = vec![];
        for value in rates.values() {
            control.checked()?;
            entries
                .push(work::clone_json(&value.value, control).map_err(CheckedError::Interrupted)?);
        }
        comparison = json!({"baseline_finished_at":baseline.document["collection"]["finished_at"],"baseline_selection_complete":baseline.document["selection"]["complete"],"method":"two_sample_linear_projection","records":entries});
    }
    let rate_ref = query.baseline.as_ref().map(|_| &rates);
    let mut selected = vec![];
    for &record in records.values() {
        control.checked()?;
        selected.push(record);
    }
    let mut rows = vec![
        row(
            "total",
            "TOTAL",
            &selected,
            &report["summary"],
            rate_ref,
            control,
        )
        .map_err(CheckedError::Interrupted)?,
    ];
    groups(
        &report["groups"],
        &records,
        rate_ref,
        "",
        &mut rows,
        control,
    )
    .map_err(CheckedError::Interrupted)?;
    let mut classes: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    let mut class_states = BTreeMap::<&str, usize>::new();
    for &record in records.values() {
        control.checked()?;
        *class_states
            .entry(record["storage_class_state"].as_str().unwrap_or("unknown"))
            .or_default() += 1;
        if record["storage_class_state"] == "known"
            && let Some(name) = record["storage_class"].as_str()
        {
            classes.entry(name).or_default().push(record);
        }
    }
    let mut class_stats = serde_json::Map::new();
    for key in ["requested", "bound", "provisioned"] {
        let (mut values, mut all_complete) = (
            vec![],
            report["selection"]["complete"] == true && !class_states.contains_key("unknown"),
        );
        for members in classes.values() {
            control.checked()?;
            let mut sum = Some(0u128);
            let mut known = 0;
            for record in members {
                control.checked()?;
                if let Some(n) = number(&record[format!("{key}_bytes")]) {
                    known += 1;
                    sum = sum.and_then(|sum| sum.checked_add(n));
                }
            }
            all_complete &= known == members.len() && sum.is_some();
            if known > 0
                && let Some(sum) = sum
            {
                values.push(sum);
            }
        }
        class_stats.insert(
            key.into(),
            distribution(values, classes.len(), all_complete, control)
                .map_err(CheckedError::Interrupted)?,
        );
    }
    let result = json!({"version":"view-storage/calculations/v1alpha1","display_unit":query.units.to_string(),"physical_total":false,"rows":rows,
        "storage_classes":{"named_count":classes.len(),"record_counts_by_assignment":class_states,"statistics":class_stats},"comparison":comparison});
    report["calculations"] = result;
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/calculations.rs"]
mod tests;
