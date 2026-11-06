//! Quality changes IR duration and supported NAM tiers, not device rate or callback size. NAM networks
//! always run at their validated native rate; no fake oversampling of weights.
pub fn load_nam(
    path: impl AsRef<std::path::Path>,
    rate: f64,
    frames: usize,
    quality: nam_bridge::NamQuality,
    alignment: bool,
) -> Result<nam_bridge::NamModel, nam_bridge::NamError> {
    if alignment {
        nam_bridge::NamModel::load_with_alignment(path, rate, frames, quality)
    } else {
        nam_bridge::NamModel::load_with_quality(path, rate, frames, quality)
    }
}

pub fn ir_seconds(profile: u8) -> f32 {
    match profile {
        0 => 0.064,
        2 => 1.0,
        _ => 0.2,
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
}
