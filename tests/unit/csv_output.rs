#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

#[test]
fn exact_flat_columns_and_quoted_unicode_newlines_round_trip() {
    let mut report: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/report.json")).unwrap();
    report["records"][0]["name"] = "alpha,\"quoted\"\nβ".into();
    let mut bytes = vec![];
    write(&mut bytes, &report, &CancellationToken::new()).unwrap();
    let mut reader = csv::Reader::from_reader(bytes.as_slice());
    assert!(reader.headers().unwrap().iter().eq(output::CSV_HEADER));
    let rows = reader.records().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 27);
    assert_eq!(&rows[0][2], "alpha,\"quoted\"\nβ");
    assert_eq!(&rows[0][9], "107374182400");
    report["records"] = serde_json::json!([]);
    let mut empty = vec![];
    write(&mut empty, &report, &CancellationToken::new()).unwrap();
    let mut reader = csv::Reader::from_reader(empty.as_slice());
    assert_eq!(reader.headers().unwrap().len(), 27);
    assert_eq!(reader.records().count(), 0);
}

#[test]
fn closed_output_pipe_remains_a_broken_pipe_error() {
    struct Closed;
    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let report: Value =
        serde_json::from_str(include_str!("../../examples/contract/healthy/report.json")).unwrap();
    assert_eq!(
        write(Closed, &report, &CancellationToken::new())
            .unwrap_err()
            .kind(),
        io::ErrorKind::BrokenPipe
    );
}
