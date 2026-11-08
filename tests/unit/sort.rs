#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

struct Budget(usize);
impl Checkpoint for Budget {
    type Error = ();
    fn step(&mut self) -> Result<(), ()> {
        self.0 = self.0.checked_sub(1).ok_or(())?;
        Ok(())
    }
}

#[test]
fn diagnostic_order_preserves_occurrences_canonicalizes_fields_and_checks_nested_work() {
    let scoped = Diagnostic {
        code: "invalid_metrics".into(),
        subject_id: Some(RecordId::new(crate::cli::Kind::Pvc, "data").unwrap()),
        source: "provider".into(),
        reason: "invalid".into(),
        affected_fields: vec!["used".into(), "available".into(), "used".into()],
        message: "bad tuple".into(),
    };
    let mut unscoped = scoped.clone();
    unscoped.subject_id = None;
    let mut values = vec![scoped.clone(), unscoped, scoped.clone()];
    diagnostics_checked(&mut values, &mut Unbounded).unwrap();
    assert!(values[0].subject_id.is_none());
    assert_eq!(
        values.len(),
        3,
        "repeated diagnostic occurrences remain inspectable"
    );
    assert!(
        values
            .iter()
            .all(|value| value.affected_fields == ["available", "used"])
    );
    assert!(diagnostics_checked(&mut vec![scoped.clone(); 1000], &mut Budget(64)).is_err());
    let mut nested = scoped;
    nested.affected_fields = (0..1000).map(|n| format!("field-{n}")).collect();
    assert!(diagnostics_checked(&mut vec![nested], &mut Budget(64)).is_err());
}

#[test]
fn unknowns_stay_last_both_ways_and_ties_ascending() {
    for descending in [false, true] {
        let mut items = [
            (Some(1), "z"),
            (None, "unknown"),
            (Some(1), "a"),
            (Some(2), "b"),
        ];
        items.sort_by(|a, b| optional(a.0, b.0, descending).then_with(|| a.1.cmp(b.1)));
        assert_eq!(items.last().unwrap().1, "unknown");
        let ties: Vec<_> = items
            .iter()
            .filter(|i| i.0 == Some(1))
            .map(|i| i.1)
            .collect();
        assert_eq!(ties, vec!["a", "z"]);
    }
}
