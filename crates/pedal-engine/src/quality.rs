//! Quality changes IR duration, NAM tiers and NAM oversampling, not device rate or callback size.
//! Every NAM instance still runs at its validated native rate: oversampling
//! phase-interleaves identical instances (see `nam_oversampling`), it never
//! rescales weights.
pub fn load_nam(
    path: impl AsRef<std::path::Path>,
    rate: f64,
    frames: usize,
    quality: nam_bridge::NamQuality,
    alignment: bool,
    oversampling: u8,
) -> Result<crate::nam_oversampling::OversampledNam, nam_bridge::NamError> {
    crate::nam_oversampling::OversampledNam::load(
        path,
        rate,
        frames,
        quality,
        alignment,
        oversampling,
    )
}

/// Offline factor: 0 means "same as live".
pub fn render_nam_oversampling(state: &crate::EngineState) -> u8 {
    if state.render_nam_oversampling == 0 {
        state.nam_oversampling
    } else {
        state.render_nam_oversampling
    }
}

pub fn ir_seconds(profile: u8) -> f32 {
    match profile {
        0 => 0.064,
        2 => 1.0,
        // Long enough for cab IRs with room/mic tails (Tone3000 caps cabs at ~0.5 s).
        _ => 0.5,
    }
}

pub fn nam_size(value: u8) -> nam_bridge::NamQuality {
    if value == 0 {
        nam_bridge::NamQuality::Lite
    } else {
        nam_bridge::NamQuality::Full
    }
}

pub fn render_nam_size(state: &crate::EngineState) -> nam_bridge::NamQuality {
    nam_size(if state.render_nam_quality == 2 {
        state.nam_quality
    } else {
        state.render_nam_quality
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn render_override_is_independent_and_old_states_default_to_full() {
        let mut s: crate::EngineState = serde_json::from_str("{}").unwrap();
        assert_eq!(s.nam_quality, 1);
        assert_eq!(s.render_nam_quality, 2);
        assert_eq!(render_nam_size(&s), nam_bridge::NamQuality::Full);
        s.nam_quality = 0;
        assert_eq!(render_nam_size(&s), nam_bridge::NamQuality::Lite);
        s.render_nam_quality = 1;
        assert_eq!(render_nam_size(&s), nam_bridge::NamQuality::Full);
        assert_eq!(s.nam_quality, 0);
    }

    #[test]
    fn oversampling_defaults_off_live_and_render_follows_live() {
        let mut s: crate::EngineState = serde_json::from_str("{}").unwrap();
        assert_eq!(s.nam_oversampling, 1);
        assert_eq!(render_nam_oversampling(&s), 1);
        s.nam_oversampling = 2;
        assert_eq!(render_nam_oversampling(&s), 2);
        s.render_nam_oversampling = 8;
        assert_eq!(render_nam_oversampling(&s), 8);
    }
}
