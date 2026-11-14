use pedal_engine::{
    dsp::{AmpProcessor, MonoProcessor, RealtimeTelemetry, SafetyProcessor},
    node_document::RigDocument,
    node_live::PatchRoute,
};
use std::sync::{Arc, atomic::AtomicBool};

#[test]
fn live_route_reports_only_the_limiter_stages_that_process_audio() {
    let telemetry = Arc::new(RealtimeTelemetry::default());
    let board = SafetyProcessor::new(AmpProcessor::Clean, Arc::clone(&telemetry), 48000);
    let (route, control) = PatchRoute::new(board, 48000, Arc::new(AtomicBool::new(false)));
    let mut monitor = SafetyProcessor::monitor(route, Arc::clone(&telemetry), 48000);
    let mut render = || {
        let mut input: Vec<_> = (0..2048)
            .map(|i| if i % 2 == 0 { 2.0 } else { -2.0 })
            .collect();
        let mut output = vec![[0.0; 2]; input.len()];
        monitor.process_stereo(&mut input, &mut output);
        assert!(
            output
                .iter()
                .flatten()
                .all(|x| x.is_finite() && x.abs() <= 1.0)
        );
    };
    render();
    let board_snapshot = telemetry.snapshot();
    assert!(board_snapshot.rig_limiter_reduction_db > 6.0);
    assert_eq!(board_snapshot.output_limiter_reduction_db, 0.0);

    let patch: RigDocument = serde_json::from_value(serde_json::json!({
        "version": 1, "sample_rate": 48000,
        "graph": {"version": 1, "nodes": [
            {"id": 1, "inputs": [], "processor": {"type": "input"}},
            {"id": 2, "inputs": [1], "processor": {"type": "output"}}
        ]}, "processors": {}
    }))
    .unwrap();
    control
        .submit(Some(patch.prepare(std::path::Path::new(".")).unwrap()))
        .unwrap();
    render(); // Finish the route transition before measuring a full patch interval.
    assert!(control.status().0);
    telemetry.snapshot();
    render();
    let patch_snapshot = telemetry.snapshot();
    assert_eq!(patch_snapshot.rig_limiter_reduction_db, 0.0);
    // The Patch Lab DC blocker trims ~0.003 dB at Nyquist.
    assert!((patch_snapshot.output_limiter_reduction_db - 6.0206).abs() < 0.01);

    control.submit(None).unwrap();
    render();
    assert!(!control.status().0);
    telemetry.snapshot();
    render();
    assert!(telemetry.snapshot().rig_limiter_reduction_db > 6.0);
}
