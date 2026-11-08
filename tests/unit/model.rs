#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
#[test]
fn identities_and_open_values() {
    assert_eq!(RecordId::new(Kind::Pv, "abc").unwrap().as_str(), "pv:abc");
    for uid in ["", "a:b", "bad\n"] {
        assert!(RecordId::new(Kind::Pvc, uid).is_err());
        assert!(ConsumerId::new(uid).is_err());
    }
    assert_eq!(
        Phase::from_api("FuturePhase"),
        Phase::Other("FuturePhase".into())
    );
    for phase in [
        "Pending",
        "Bound",
        "Lost",
        "Available",
        "Released",
        "Failed",
        "SomethingNew",
    ] {
        assert_eq!(Phase::from_api(phase).as_str(), phase);
    }
    assert_eq!(MetadataValue::<String>::Absent.state(), "absent");
    assert_eq!(MetadataValue::<String>::Unknown.state(), "unknown");
    assert_eq!(MetadataValue::Known("fast").known(), Some(&"fast"));
}
#[test]
fn three_valued_and_never_promotes_unknown() {
    let unknown = Predicate::Unknown(BTreeSet::from(["node".into()]));
    assert_eq!(unknown.clone().and(Predicate::NoMatch), Predicate::NoMatch);
    assert_eq!(Predicate::Match.and(unknown.clone()), unknown);
    assert_eq!(
        unknown
            .clone()
            .and(Predicate::Unknown(BTreeSet::from(["driver".into()]))),
        Predicate::Unknown(BTreeSet::from(["node".into(), "driver".into()]))
    );
}
