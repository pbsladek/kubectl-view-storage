#![no_main]
use kubectl_view_storage::qty::{Bytes, Threshold};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(input) = std::str::from_utf8(data) {
        if let Ok(value) = input.parse::<Bytes>() {
            assert_eq!(
                value.get().to_string().parse::<Bytes>().map(Bytes::get),
                Ok(value.get())
            );
        }
        if let Ok(threshold) = input.parse::<Threshold>() {
            assert_eq!(threshold.to_string().parse::<Threshold>(), Ok(threshold));
        }
    }
});
