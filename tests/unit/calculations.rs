#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::{cli::Cli, work::Unbounded};
use clap::Parser;

fn document() -> Value {
    serde_json::from_str(include_str!("../../examples/contract/healthy/report.json")).unwrap()
}
fn query(args: &[&str]) -> Query {
    Cli::try_parse_from(
        ["cmd", "--view", "stats"]
            .into_iter()
            .chain(args.iter().copied()),
    )
    .unwrap()
    .query("demo")
    .unwrap()
}
fn compare_pair() -> (Value, Value) {
    let old = document();
    let mut current = old.clone();
    current["collection"]["finished_at"] = json!("2026-11-06T21:00:00Z");
    current["records"][0]["filesystem"]["observed_at"] = json!("2026-11-06T20:59:50Z");
    current["records"][0]["filesystem"]["used_bytes"] = json!("44023414784");
    current["records"][0]["filesystem"]["available_bytes"] = json!("57982058496");
    (old, current)
}

#[test]
fn decimal_and_binary_units_are_exact_case_insensitive_and_safe_at_integer_limits() {
    assert_eq!(
        "gb".parse::<Unit>().unwrap().format(1_000_000_000, 1),
        "1.00 GB"
    );
    assert_eq!(
        "GiB".parse::<Unit>().unwrap().format(1_073_741_824, 1),
        "1.00 GiB"
    );
    assert_eq!(Unit::Auto.format(1_073_741_824, 1), "1.00 GiB");
    assert_eq!(Unit::Mb.format(1_000_000_000, 1), "1000.00 MB");
    assert_eq!(Unit::Tb.format(1_000_000_000_000, 1), "1.00 TB");
    assert_eq!(Unit::Tb.format(1_073_741_824, 1), "0.001074 TB");
    assert_eq!(Unit::Pb.format(1, 1), "<0.000001 PB");
    assert_eq!(Unit::B.format(u128::MAX, 1), format!("{}.00 B", u128::MAX));
    assert_eq!(Unit::B.format(1, 0), "--");
    assert_eq!(Unit::B.format(1, 8), "0.13 B");
    assert_eq!(Unit::B.format(u128::MAX - 1, u128::MAX), "1.00 B");
    assert!("xb".parse::<Unit>().is_err());
    for unit in Unit::ALL {
        assert_eq!(unit.to_string().parse::<Unit>().unwrap(), unit);
    }
}

#[test]
fn distributions_preserve_large_sums_fractional_mean_median_and_nearest_rank() {
    let result = distribution(vec![1, 2, 3, 100], 4, true, &mut Unbounded).unwrap();
    assert_eq!(result["sum_bytes"], "106");
    assert_eq!(
        result["mean_bytes"],
        json!({"numerator":"53","denominator":"2"})
    );
    assert_eq!(
        result["median_bytes"],
        json!({"numerator":"5","denominator":"2"})
    );
    assert_eq!(result["p95_bytes"], "100");
    let values = (1..=20).collect();
    assert_eq!(
        distribution(values, 20, true, &mut Unbounded).unwrap()["p95_bytes"],
        "19"
    );
    let large = distribution(vec![u64::MAX as u128; 3], 3, true, &mut Unbounded).unwrap();
    assert_eq!(large["sum_bytes"], (3 * u64::MAX as u128).to_string());
    assert_eq!(large["complete"], true);
    let partial = distribution(vec![0], 2, true, &mut Unbounded).unwrap();
    assert_eq!(partial["sum_bytes"], "0");
    assert_eq!(partial["complete"], false);
    let unknown = distribution(vec![], 2, true, &mut Unbounded).unwrap();
    assert!(unknown["sum_bytes"].is_null());
    assert!(unknown["median_bytes"].is_null());
    let empty = distribution(vec![], 0, true, &mut Unbounded).unwrap();
    assert_eq!(empty["sum_bytes"], "0");
    assert_eq!(empty["complete"], true);
    let overflow = distribution(vec![u128::MAX, 1], 2, true, &mut Unbounded).unwrap();
    assert!(overflow["sum_bytes"].is_null());
    assert_eq!(overflow["complete"], false);
}

#[test]
fn class_statistics_are_distributions_of_class_totals_and_distinguish_unknown_classes() {
    let mut report = document();
    report["groups"] = json!([]);
    let mut records = vec![];
    for (index, (class, amount)) in [
        (Some("fast"), 10),
        (Some("fast"), 30),
        (Some("slow"), 60),
        (None, 1000),
    ]
    .into_iter()
    .enumerate()
    {
        let mut record = report["records"][0].clone();
        record["id"] = json!(format!("pvc:{index}"));
        record["storage_class"] = json!(class);
        record["storage_class_state"] = json!(if class.is_some() {
            "known"
        } else {
            "classless"
        });
        record["requested_bytes"] = json!(amount.to_string());
        records.push(record);
    }
    report["records"] = json!(records);
    attach_checked(&mut report, &query(&[]), &mut Unbounded).unwrap();
    let calcs = &report["calculations"];
    assert_eq!(
        calcs["rows"][0]["statistics"]["requested"]["sum_bytes"],
        "1100"
    );
    assert_eq!(calcs["storage_classes"]["named_count"], 2);
    let stats = &calcs["storage_classes"]["statistics"]["requested"];
    assert_eq!(stats["sum_bytes"], "100");
    assert_eq!(stats["min_bytes"], "40");
    assert_eq!(stats["max_bytes"], "60");
    assert_eq!(stats["mean_bytes"]["numerator"], "50");
    assert_eq!(calcs["physical_total"], false);
}

#[test]
fn missing_quantities_gaps_and_incomplete_selection_do_not_fabricate_completeness() {
    let mut report = document();
    report["summary"]["selection_complete"] = false.into();
    report["records"][0]["requested_bytes"] = "120".into();
    report["records"][0]["bound_bytes"] = "100".into();
    report["records"][0]["provisioned_bytes"] = "150".into();
    attach_checked(&mut report, &query(&[]), &mut Unbounded).unwrap();
    let gaps = &report["calculations"]["rows"][0]["capacity_gaps"];
    assert_eq!(gaps["requested_above_bound_bytes"], "20");
    assert_eq!(gaps["provisioned_above_requested_bytes"], "30");
    assert_eq!(gaps["complete"], false);
    report["records"][0]["requested_bytes"] = Value::Null;
    attach_checked(&mut report, &query(&[]), &mut Unbounded).unwrap();
    assert!(
        report["calculations"]["rows"][0]["capacity_gaps"]["requested_above_bound_bytes"].is_null()
    );
    assert!(report["calculations"]["rows"][0]["statistics"]["requested"]["sum_bytes"].is_null());
}

#[test]
fn rates_use_sample_time_available_bytes_and_preserve_signed_change() {
    let (old, current) = compare_pair();
    let previous = BTreeMap::from([("pvc:data", &old["records"][0])]);
    let result = rate(&current["records"][0], previous.get("pvc:data"));
    assert_eq!(result.value["status"], "growing");
    assert_eq!(
        format_hourly_rate(&result.value["growth_bytes_per_second"], Unit::Gib),
        "1.00 GiB"
    );
    assert_eq!(
        format_seconds(&result.value["time_to_full_seconds"]),
        "2.25 d"
    );
    let reverse = rate(&old["records"][0], Some(&&current["records"][0]));
    assert_eq!(reverse.value["reason"], "sample_not_newer");
    let mut shrinking = current.clone();
    shrinking["records"][0]["filesystem"]["used_bytes"] = "41875931136".into();
    let result = rate(&shrinking["records"][0], previous.get("pvc:data"));
    assert_eq!(result.value["status"], "shrinking");
    assert_eq!(
        format_hourly_rate(&result.value["growth_bytes_per_second"], Unit::Gib),
        "-1.00 GiB"
    );
    assert!(result.value["time_to_full_seconds"].is_null());
    let mut steady = current;
    steady["records"][0]["filesystem"]["used_bytes"] =
        old["records"][0]["filesystem"]["used_bytes"].clone();
    let result = rate(&steady["records"][0], previous.get("pvc:data"));
    assert_eq!(result.value["status"], "steady");
    assert!(result.value["time_to_full_seconds"].is_null());
}

#[test]
fn growth_rejects_new_rebound_resized_conflicting_stale_block_and_repeated_samples() {
    let (old, current) = compare_pair();
    let previous = &&old["records"][0];
    assert_eq!(
        rate(&current["records"][0], None).value["reason"],
        "no_previous_identity"
    );
    for (pointer, value, reason) in [
        (
            "/pv_name",
            json!("replacement"),
            "identity_or_binding_changed",
        ),
        (
            "/created_at",
            json!("2026-11-05T00:00:00Z"),
            "identity_or_binding_changed",
        ),
        (
            "/filesystem/capacity_bytes",
            json!("120000000000"),
            "capacity_changed",
        ),
        (
            "/filesystem/status",
            json!("stale"),
            "incomplete_or_unusable_samples",
        ),
        (
            "/filesystem/status",
            json!("uncertain"),
            "incomplete_or_unusable_samples",
        ),
        (
            "/overlapping_filesystem",
            json!(true),
            "incomplete_or_unusable_samples",
        ),
        (
            "/filesystem/observed_at",
            old["records"][0]["filesystem"]["observed_at"].clone(),
            "sample_not_newer",
        ),
        (
            "/filesystem/used_bytes",
            json!("18446744073709551616"),
            "incomplete_or_unusable_samples",
        ),
    ] {
        let mut record = current["records"][0].clone();
        if pointer == "/overlapping_filesystem" {
            record["overlapping_filesystem"] = value;
        } else {
            *record.pointer_mut(pointer).unwrap() = value;
        }
        assert_eq!(
            rate(&record, Some(previous)).value["reason"],
            reason,
            "{pointer}"
        );
    }
    let mut block = old["records"][0].clone();
    block["volume_mode"] = "Block".into();
    assert!(sample(&block).is_none());
}

#[test]
fn pv_growth_requires_the_same_verified_claim_in_both_reports() {
    let (old, current) = compare_pair();
    let mut old = old["records"][0].clone();
    let mut current = current["records"][0].clone();
    for record in [&mut old, &mut current] {
        record["kind"] = "pv".into();
        record["pvc_name"] = "data".into();
        record["pvc_uid"] = "original-claim".into();
    }
    assert_eq!(rate(&current, Some(&&old)).value["status"], "growing");
    for (key, value) in [("pvc_name", "replacement"), ("pvc_uid", "replacement-uid")] {
        let mut rebound = current.clone();
        rebound[key] = value.into();
        let result = rate(&rebound, Some(&&old));
        assert_eq!(result.value["reason"], "identity_or_binding_changed");
        assert!(result.rate.is_none());
    }
    for key in ["pvc_name", "pvc_uid"] {
        old.as_object_mut().unwrap().remove(key);
        current.as_object_mut().unwrap().remove(key);
    }
    let result = rate(&current, Some(&&old));
    assert_eq!(result.value["reason"], "incomplete_or_unusable_samples");
    assert!(result.rate.is_none());
}

#[test]
fn comparisons_validate_scope_and_schema_without_trusting_totals() {
    let (old, current) = compare_pair();
    let baseline = Baseline::from_document(old.clone()).unwrap();
    baseline.validate_scope(&current).unwrap();
    for pointer in [
        "/collection/context",
        "/query/kind",
        "/collection/scope/namespace",
        "/query/filters/node",
    ] {
        let mut changed = current.clone();
        *changed.pointer_mut(pointer).unwrap() = json!("different");
        assert!(baseline.validate_scope(&changed).is_err());
    }
    assert!(baseline.validate_scope(&old).is_err());
    for pointer in [
        "/schema_version",
        "/query/utilization",
        "/records/0/id",
        "/records/0/uid",
        "/records/0/filesystem",
        "/collection/finished_at",
    ] {
        let mut invalid = old.clone();
        *invalid.pointer_mut(pointer).unwrap() = Value::Null;
        assert!(Baseline::from_document(invalid).is_err(), "{pointer}");
    }
    let mut duplicates = old;
    let duplicate = duplicates["records"][0].clone();
    duplicates["records"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert!(Baseline::from_document(duplicates).is_err());
}

#[test]
fn growth_sums_paired_claim_rates_and_uses_the_first_claim_to_fill() {
    let (old, mut current) = compare_pair();
    let mut q = query(&["-u"]);
    q.baseline = Some(std::sync::Arc::new(Baseline::from_document(old).unwrap()));
    attach_checked(&mut current, &q, &mut Unbounded).unwrap();
    let growth = &current["calculations"]["rows"][0]["growth"];
    assert_eq!(growth["complete"], true);
    assert_eq!(growth["matched_count"], 1);
    assert_eq!(
        format_hourly_rate(&growth["growth_bytes_per_second"], Unit::Gib),
        "1.00 GiB"
    );
    assert_eq!(format_seconds(&growth["first_full_seconds"]), "2.25 d");
    assert_eq!(
        current["calculations"]["comparison"]["records"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let mut second = current["records"][0].clone();
    second["id"] = "pvc:new".into();
    second["uid"] = "new".into();
    current["records"].as_array_mut().unwrap().push(second);
    attach_checked(&mut current, &q, &mut Unbounded).unwrap();
    assert_eq!(
        current["calculations"]["rows"][0]["growth"]["complete"],
        false
    );
    let sum = Fraction::new(false, 3, 2)
        .unwrap()
        .add(Fraction::new(true, 1, 3).unwrap())
        .unwrap();
    assert_eq!(sum.value(), json!({"numerator":"7","denominator":"6"}));
    assert!(
        Fraction::new(false, u128::MAX, 1)
            .unwrap()
            .add(Fraction::new(false, 1, 1).unwrap())
            .is_none()
    );
}

#[test]
fn calculation_processing_cooperates_with_cancellation() {
    struct Stop;
    impl Checkpoint for Stop {
        type Error = &'static str;
        fn step(&mut self) -> Result<(), Self::Error> {
            Err("stop")
        }
    }
    let mut report = document();
    assert!(matches!(
        attach_checked(&mut report, &query(&[]), &mut Stop),
        Err(CheckedError::Interrupted("stop"))
    ));
    assert!(report.get("calculations").is_none());
}

#[test]
fn projection_overflow_is_explicit_and_block_rates_are_not_applicable() {
    let mut old = document()["records"][0].clone();
    old["filesystem"]["capacity_bytes"] = u64::MAX.to_string().into();
    old["filesystem"]["used_bytes"] = "0".into();
    old["filesystem"]["available_bytes"] = u64::MAX.to_string().into();
    old["filesystem"]["observed_at"] = "0001-01-01T00:00:00.000000001Z".into();
    let mut current = old.clone();
    current["filesystem"]["used_bytes"] = "1".into();
    current["filesystem"]["available_bytes"] = (u64::MAX - 1).to_string().into();
    current["filesystem"]["observed_at"] = "9999-01-01T00:00:00Z".into();
    let result = rate(&current, Some(&&old));
    assert_eq!(result.value["status"], "growing");
    assert_eq!(result.value["reason"], "projection_overflow");
    assert!(result.full.is_none());
    let rates = BTreeMap::from([("pvc:data", result)]);
    let group = growth(&[&current], &rates, true, &mut Unbounded).unwrap();
    assert_eq!(group["complete"], false);
    assert_eq!(group["reason"], "arithmetic_overflow");
    current["volume_mode"] = "Block".into();
    assert_eq!(rate(&current, None).value["status"], "not_applicable");
    assert_eq!(
        format_hourly_rate(
            &json!({"numerator":"-1","denominator":"1000000000000000"}),
            Unit::Pb
        ),
        "decrease <0.000001 PB"
    );
}

#[test]
fn baseline_files_are_bounded_regular_json_and_never_block_on_fifos() {
    struct Directory(std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let root = Directory(std::env::temp_dir().join(format!(
            "view-storage-comparison-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    std::fs::create_dir(&root.0).unwrap();
    let path = root.0.join("report.json");
    assert!(Baseline::load(&path).is_err());
    assert!(Baseline::load(&root.0).is_err());
    std::fs::write(&path, serde_json::to_vec(&document()).unwrap()).unwrap();
    Baseline::load(&path).unwrap();
    std::fs::write(&path, b"not JSON").unwrap();
    assert!(Baseline::load(&path).unwrap_err().contains("not valid"));
    std::fs::File::create(&path)
        .unwrap()
        .set_len(Baseline::MAX_BYTES + 1)
        .unwrap();
    assert!(Baseline::load(&path).unwrap_err().contains("32 MiB"));
    #[cfg(unix)]
    {
        let fifo = root.0.join("fifo");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRUSR).unwrap();
        assert!(Baseline::load(&fifo).unwrap_err().contains("regular file"));
    }
}

#[test]
fn group_growth_selects_earliest_full_claim_even_when_other_claims_shrink() {
    let mut current = document();
    let first = current["records"][0].clone();
    let mut second = first.clone();
    second["id"] = "pvc:second".into();
    let rates = BTreeMap::from([
        (
            "pvc:data",
            Rate {
                value: json!({}),
                rate: Fraction::new(false, 10, 1),
                full: Fraction::new(false, 100, 1),
            },
        ),
        (
            "pvc:second",
            Rate {
                value: json!({}),
                rate: Fraction::new(true, 12, 1),
                full: None,
            },
        ),
    ]);
    let result = growth(&[&first, &second], &rates, true, &mut Unbounded).unwrap();
    assert_eq!(
        result["growth_bytes_per_second"],
        json!({"numerator":"-2","denominator":"1"})
    );
    assert_eq!(
        result["first_full_seconds"],
        json!({"numerator":"100","denominator":"1"})
    );
    assert_eq!(result["complete"], true);
    let mut faster = rates;
    faster.get_mut("pvc:second").unwrap().full = Fraction::new(false, 5, 1);
    assert_eq!(
        growth(&[&first, &second], &faster, true, &mut Unbounded).unwrap()["first_full_seconds"]["numerator"],
        "5"
    );
    current["records"][0]["volume_mode"] = "Block".into();
    assert!(growth(&[&current["records"][0]],&faster,true,&mut Unbounded).unwrap()["growth_bytes_per_second"].is_null());
    assert_eq!(
        Fraction::new(false, 0, 1).unwrap().forecast(0).unwrap().n,
        0
    );
    assert_eq!(
        Fraction::new(false, 1, 4)
            .unwrap()
            .add(Fraction::new(true, 2, 1).unwrap())
            .unwrap()
            .value(),
        json!({"numerator":"-7","denominator":"4"})
    );
    assert_eq!(
        Fraction::new(true, 1, 1)
            .unwrap()
            .add(Fraction::new(true, 1, 1).unwrap())
            .unwrap()
            .value(),
        json!({"numerator":"-2","denominator":"1"})
    );
    assert_eq!(
        format_seconds(&json!({"numerator":"2","denominator":"1"})),
        "2.00 s"
    );
    assert_eq!(
        format_seconds(&json!({"numerator":"120","denominator":"1"})),
        "2.00 m"
    );
    assert_eq!(
        format_seconds(&json!({"numerator":"3600","denominator":"1"})),
        "1.00 h"
    );
    assert_eq!(format_seconds(&Value::Null), "--");
}
