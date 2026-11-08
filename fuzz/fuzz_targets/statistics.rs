#![no_main]
use kubectl_view_storage::{
    cli::Kind,
    metrics,
    model::{ConsumerId, Eligibility, RecordId, Source},
    qty::{Bytes, Inodes},
    transport,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(value) = transport::decode_json(data) {
        let source = Source {
            node: "node".into(),
            pod_id: ConsumerId::new("pod").expect("fixed valid UID"),
            volume_name: "data".into(),
        };
        let id = RecordId::new(Kind::Pvc, "claim").expect("fixed valid UID");
        let mut diagnostics = Vec::new();
        let _ = metrics::normalize_candidate::<Bytes>(
            &value,
            ["capacityBytes", "usedBytes", "availableBytes"],
            source.clone(),
            None,
            Eligibility::Fresh,
            &id,
            &mut diagnostics,
        );
        let _ = metrics::normalize_candidate::<Inodes>(
            &value,
            ["inodes", "inodesUsed", "inodesFree"],
            source,
            None,
            Eligibility::Fresh,
            &id,
            &mut diagnostics,
        );
    }
});
