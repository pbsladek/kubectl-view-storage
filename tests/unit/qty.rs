#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

#[test]
fn quantities_cover_suffixes_and_round_up() {
    for (input, expected) in [
        ("0", 0),
        ("1", 1),
        ("+1", 1),
        ("1.1", 2),
        (".1", 1),
        ("1.", 1),
        ("1Ki", 1024),
        ("1.5Gi", 1610612736),
        ("1m", 1),
        ("1u", 1),
        ("1n", 1),
        ("1k", 1000),
        ("1K", 1000),
        ("1M", 1000000),
        ("1G", 1000000000),
        ("1T", 1000000000000),
        ("1P", 1000000000000000),
        ("1E", 1000000000000000000),
        ("1Mi", 1048576),
        ("1Ti", 1099511627776),
        ("1Pi", 1125899906842624),
        ("1Ei", 1152921504606846976),
        ("1e3", 1000),
        ("1E+3", 1000),
        ("1e-999999999999999999", 1),
        ("0001.0000", 1),
        ("0e9999999999999999", 0),
        ("18446744073709551615", u64::MAX),
        ("18446744073709551614.1", u64::MAX),
    ] {
        assert_eq!(
            input.parse::<Bytes>().map(Bytes::get),
            Ok(expected),
            "{input}"
        );
    }
}
#[test]
fn quantity_errors_are_explicit() {
    for input in [
        "", ".", " 1", "1 ", "1e", "1e--1", "1e+", "NaN", "inf", "1ki", "1..2", "++1", "1e1x",
    ] {
        assert_eq!(
            input.parse::<Bytes>(),
            Err(QuantityError::Syntax),
            "{input}"
        );
    }
    assert_eq!("-1".parse::<Bytes>(), Err(QuantityError::Negative));
    for input in [
        "18446744073709551616",
        "18446744073709551615.1",
        "1e9999999999999999999",
    ] {
        assert_eq!(input.parse::<Bytes>(), Err(QuantityError::OutOfRange));
    }
    assert_eq!(
        "1".repeat(257).parse::<Bytes>(),
        Err(QuantityError::TooLong)
    );
    assert_eq!(checked_sum([u128::MAX, 1]), Err(QuantityError::OutOfRange));
    assert_eq!(checked_sum([u128::MAX, 0]), Ok(u128::MAX));
    assert_eq!(checked_sum([]), Ok(0));
    let mut input = "1".to_owned();
    input.push_str(&"0".repeat(240));
    input.push_str("e-240");
    assert_eq!(input.parse::<Bytes>(), Ok(Bytes::new(1)));
}
#[test]
fn thresholds_are_exact_and_canonical() {
    for (input, canonical) in [
        ("80.000000", "80"),
        ("080.010000", "80.01"),
        ("0.000001", "0.000001"),
        ("100", "100"),
    ] {
        assert_eq!(input.parse::<Threshold>().unwrap().to_string(), canonical);
    }
    for input in [
        "0",
        "100.000001",
        "-1",
        "1e1",
        "NaN",
        ".5",
        "5.",
        "1.0000000",
        "",
    ] {
        assert!(input.parse::<Threshold>().is_err(), "{input}");
    }
    let threshold: Threshold = "80".parse().unwrap();
    assert!(!threshold.reached(799999999999999999, 1000000000000000000));
    assert!(threshold.reached(800000000000000000, 1000000000000000000));
    assert!(!threshold.reached(0, 0));
    assert!("0.9".parse::<Threshold>().unwrap() < "1".parse::<Threshold>().unwrap());
}
#[test]
fn canonical_ratios_and_extreme_ordering() {
    assert_eq!(ExactRatio::new(2, 4), ExactRatio::new(1, 2));
    assert_eq!(ExactRatio::new(0, 7), ExactRatio::new(0, 1));
    assert_eq!(ExactRatio::new(1, 0), Err(QuantityError::ZeroDenominator));
    let m = u128::MAX;
    assert!(ExactRatio::new(m - 1, m).unwrap() > ExactRatio::new(m - 2, m - 1).unwrap());
    assert!(ExactRatio::new(m, m - 1).unwrap() < ExactRatio::new(m - 1, m - 2).unwrap());
    for (n, d, display) in [
        (0, 1, "0"),
        (1, 3, "33.333333"),
        (2, 3, "66.666667"),
        (1, 1, "100"),
        (m - 1, m, "100"),
        (1, m, "0"),
        (1, 200000000, "0.000001"),
    ] {
        assert_eq!(ExactRatio::new(n, d).unwrap().percent(), display);
    }
    for a in 0..30u128 {
        for b in 1..30u128 {
            for c in 0..30u128 {
                for d in 1..30u128 {
                    let left = ExactRatio::new(a, b).unwrap();
                    let right = ExactRatio::new(c, d).unwrap();
                    assert_eq!(left.cmp(&right), (a * d).cmp(&(c * b)));
                    assert_eq!(left == right, left.cmp(&right) == Ordering::Equal);
                }
            }
        }
    }
}
