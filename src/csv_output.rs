//! CSV escaping and record-length validation use the maintained csv crate.
use kubectl_view_storage::output;
use serde_json::Value;
use std::io::{self, Write};
use tokio_util::sync::CancellationToken;

pub fn write(writer: impl Write, report: &Value, cancel: &CancellationToken) -> io::Result<()> {
    check_cancel(cancel)?;
    let mut writer = csv::WriterBuilder::new().from_writer(CancelWriter { writer, cancel });
    let header: &[&str] = if report.get("calculations").is_some() {
        &output::CALCULATION_CSV_HEADER
    } else {
        &output::CSV_HEADER
    };
    writer.write_record(header).map_err(csv_error)?;
    output::for_each_csv_row(report, cancel, |row| {
        writer.write_record(row).map_err(csv_error)
    })?;
    writer.flush()
}

// csv::Writer also buffers and flushes on Drop. Keep cancellation effective
// beneath that buffer even when used outside the command's stdio adapter.
struct CancelWriter<'a, W> {
    writer: W,
    cancel: &'a CancellationToken,
}
impl<W: Write> Write for CancelWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        check_cancel(self.cancel)?;
        self.writer.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        check_cancel(self.cancel)?;
        self.writer.flush()
    }
}
fn check_cancel(cancel: &CancellationToken) -> io::Result<()> {
    if cancel.is_cancelled() {
        Err(io::Error::other("run interrupted"))
    } else {
        Ok(())
    }
}

fn csv_error(error: csv::Error) -> io::Error {
    match error.into_kind() {
        csv::ErrorKind::Io(error) => error,
        _ => io::Error::new(io::ErrorKind::InvalidData, "CSV serialization failed"),
    }
}

#[cfg(test)]
#[path = "../tests/unit/csv_output.rs"]
mod tests;
