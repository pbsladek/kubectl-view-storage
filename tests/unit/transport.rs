#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
#[test]
fn limits_and_unknown_fields() {
    assert!(decode_json(br#"{"extra":{"unknown":3},"text":"{\"["}"#).is_ok());
    assert!(decode_json(b"{broken").is_err());
    let deep = format!("{}0{}", "[".repeat(129), "]".repeat(129));
    assert_eq!(
        check_json_limits(deep.as_bytes()).unwrap_err(),
        "json_too_deep"
    );
    assert!(check_json_limits(format!("[{}]", "9".repeat(256)).as_bytes()).is_ok());
    assert!(check_json_limits(format!("[{}]", "9".repeat(257)).as_bytes()).is_err());
    assert!(check_json_limits(&vec![b' '; MAX_RESPONSE_BYTES + 1]).is_err());
    let object: Object=serde_json::from_str(r#"{"metadata":{"name":"x","uid":"id","namespace":"demo"},"unknown":true,"status":{"phase":"FuturePhase"}}"#).unwrap();
    assert_eq!(object.uid().unwrap(), "id");
    assert_eq!(object.namespace().unwrap(), "demo");
    assert!(serde_json::from_str::<Object>(r#"{"metadata":{"uid":"x"}}"#).is_err());
}

#[test]
fn exact_nesting_limit_accepts_boundary_and_rejects_excess() {
    let boundary = format!(
        "{}0{}",
        "[".repeat(MAX_JSON_DEPTH),
        "]".repeat(MAX_JSON_DEPTH)
    );
    assert!(
        decode_json(boundary.as_bytes()).is_ok(),
        "depth 128 must remain supported"
    );
    let excess = format!(
        "{}0{}",
        "[".repeat(MAX_JSON_DEPTH + 1),
        "]".repeat(MAX_JSON_DEPTH + 1)
    );
    assert_eq!(decode_json(excess.as_bytes()).unwrap_err(), "json_too_deep");
    assert!(
        decode_json(b"{} {}").is_err(),
        "a complete response cannot contain trailing JSON documents"
    );
}

struct Budget(usize);
impl Checkpoint for Budget {
    type Error = &'static str;
    fn step(&mut self) -> Result<(), Self::Error> {
        self.0 = self.0.checked_sub(1).ok_or("cancelled")?;
        Ok(())
    }
}

#[test]
fn checked_reader_and_slice_parser_preserve_the_same_json_values_and_limits() {
    let boundary = format!(
        "{}0{}",
        "[".repeat(MAX_JSON_DEPTH),
        "]".repeat(MAX_JSON_DEPTH)
    );
    let unicode =
        serde_json::json!({"unknown": "β\n\\\"".repeat(DECODE_CHUNK), "nested":[{},null,true,0]})
            .to_string();
    for input in [
        r#"{"n":184467440737095516160,"zero":0,"float":1.234567890123456789,"exponent":1e10000}"#,
        &boundary,
        &unicode,
    ] {
        let mut reference = serde_json::Deserializer::from_slice(input.as_bytes());
        reference.disable_recursion_limit();
        let expected = Value::deserialize(&mut reference).unwrap();
        reference.end().unwrap();
        assert_eq!(
            decode_json_checked(input.as_bytes(), &mut Budget(usize::MAX)).unwrap(),
            expected
        );
        assert_eq!(decode_json(input.as_bytes()).unwrap(), expected);
    }
    for input in [
        "{} {}".to_owned(),
        "{broken".into(),
        format!("[{}]", "9".repeat(257)),
        format!("{}0{}", "[".repeat(129), "]".repeat(129)),
    ] {
        let expected = decode_json(input.as_bytes()).unwrap_err();
        assert!(
            matches!(decode_json_checked(input.as_bytes(), &mut Budget(usize::MAX)), Err(CheckedError::Invalid(message)) if message == expected)
        );
    }
}

#[test]
fn lexical_and_parser_work_can_stop_inside_a_long_unknown_string() {
    let input = format!(r#"{{"unknown":"{}","usable":0}}"#, "x".repeat(1024 * 1024));
    assert!(matches!(
        check_json_limits_checked(input.as_bytes(), &mut Budget(2)),
        Err(CheckedError::Interrupted("cancelled"))
    ));
    check_json_limits(input.as_bytes()).unwrap();
    // Isolate the actual Serde reader phase after the lexical guard. A whole
    // from_slice parse with checks only around it would incorrectly succeed.
    assert!(matches!(
        parse_json_checked(input.as_bytes(), &mut Budget(2)),
        Err(CheckedError::Interrupted("cancelled"))
    ));
    let mut budget = Budget(10);
    let mut reader = CheckedReader {
        input: input.as_bytes(),
        control: &mut budget,
        stopped: None,
    };
    assert_eq!(reader.read(&mut []).unwrap(), 0);
    let mut bytes = vec![0; DECODE_CHUNK * 2];
    assert_eq!(reader.read(&mut bytes).unwrap(), DECODE_CHUNK);
    assert_eq!(reader.control.0, 9);
    let mut budget = Budget(0);
    let mut reader = CheckedReader {
        input: b"{}",
        control: &mut budget,
        stopped: None,
    };
    assert_ne!(
        reader.read(&mut bytes).unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
    assert!(reader.read(&mut bytes).is_err());
    assert_eq!(
        reader.input, b"{}",
        "interrupted reads must not consume more bytes"
    );
}
