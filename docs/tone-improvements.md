# Tone improvement rollout

Implement and verify one stage at a time. Priority balances audible benefit,
portability, and implementation risk:

1. Musical gate controls and regression tests (implemented).
2. A2 Lite / Full model selection for supported captures (implemented; see
   [NAM model quality](nam-model-quality.md)).
3. Advanced pre/post NAM EQ (implemented; see [NAM EQ](nam-eq.md)).
4. Confidence-checked dry/wet capture-delay compensation (implemented; see
   [dry/wet alignment](dry-wet-alignment.md)).
5. Architecture-aware optional oversampling.

Regression coverage accompanies every stage rather than waiting until the end.
Parallel routing, library enrichment, MIDI, and other feature work are separate.

## Gate

- Default threshold is −80 dB in new rigs, missing-field fallbacks, and the
  base AI tone recipe. Saved thresholds are not migrated. The AI's explicit
  high-gain recipe can still choose a stronger threshold (currently −48 dB).

- Gate panel retains Threshold and adds Hold, Release, and Reduction vertical
  sliders. Controls persist in presets and offline engine state.
- Tight Rhythm (10 ms / 15 ms / 80 dB), Natural Sustain (60 / 150 / 80),
  and Gentle Cleanup (30 / 200 / 25) change hold/release/reduction only.
  They do not alter threshold or turn on a bypassed pedal.
- Existing presets without the new fields get 20 ms hold, 50 ms release, and
  80 dB reduction. Existing threshold and enabled state are preserved. This is
  an intentional universal algorithm update, not an exact legacy sound mode.
- The detector alone uses a gentle 40 Hz high-pass and 5 kHz low-pass. This
  reduces DC/rumble and high-frequency hiss sensitivity without filtering the
  audible guitar. It is not a mains-hum notch or an automatic threshold setter.
- Peak envelope, 6 dB hysteresis, consecutive hold, a 4:1 closing expander,
  attenuation floor, 0.5 ms attack, and smoothed release. Release is a time
  constant, not a promise to reach silence in that exact duration.
- Clean-input detection still controls a linked gain after NAM/cabinet and
  before spatial effects. Model-generated hiss is reduced; reverb/delay tails
  are not directly gated. Clean-octave latency still extends the hold.
- No lookahead, new audio buffering, callback locks, or sample-path allocation.
  Controls are bounded defensively on the DSP side as well as at the API.

The implementation is original Rust using standard envelope/filter techniques;
TONE3000's gate was used as a behavioral reference, not copied JUCE code.

### Verification

- 98 pedal-engine tests pass (library, control API, and integration suites).
- 57 Flutter tests pass, including gate controls at 800×600 and 480×320.
- Flutter analysis and pedal-engine all-target Clippy pass without warnings.
- Gate regressions cover attenuation depth, hold/release behavior, low guitar
  notes, DC rejection, nonfinite inputs, smooth bypass, stereo gain linking,
  callback-size invariance, unmodified NAM input, and live parameter updates.
- Preset round-trip, legacy defaults, and offline state persistence are checked.
- Physical guitar listening and Raspberry Pi performance are not yet verified.
