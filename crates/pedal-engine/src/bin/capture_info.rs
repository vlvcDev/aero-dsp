//! `pedal-capture-info <model.nam>...`: one JSON object per line with each
//! capture's gear type, whether it includes a cabinet, and its small-signal
//! gain (dB, for a -80 dBFS noise input; null if it could not be measured).
//! Used by the catalog service to annotate the model browser.
fn main() {
    for argument in std::env::args().skip(1) {
        let path = std::path::Path::new(&argument);
        let gain = pedal_engine::capture_info::small_signal_gain_db(path).ok();
        let line = serde_json::json!({
            "path": argument,
            "gear_type": pedal_engine::capture_info::gear_type(path),
            "includes_cab": pedal_engine::capture_info::includes_cab(path),
            "gain_db": gain.map(|g| (f64::from(g) * 10.0).round() / 10.0),
        });
        println!("{line}");
    }
}
