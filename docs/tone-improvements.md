# Tone improvement rollout

## Cabinet IR levels (November 12, 2026)

All cabinet WAV loaders (pedalboard, Patch Lab, and offline renders) now correct
resampled filter coefficients by source rate / engine rate. Ordinary audio
resampling stays unchanged. This prevents sample-rate-dependent cabinet gain.

The retained, tapered kernel gets attenuation-only energy normalization, referred
to 48 kHz, followed by a fixed −18 dB cabinet pad. Normalization never boosts quiet
IRs. Stereo channels share the gain calculated from the higher-energy channel;
this preserves their balance and avoids making a duplicated mono IR 3 dB quieter.
This differs deliberately from TONE3000's summed-channel energy normalization.
Preparation happens during loading, with no added sample-path work or latency.

This follows TONE3000's cabinet headroom approach (reference commit
`1bbb21b5630bf359c348c4f16ffda7a47695db2d`, `ProcessorModelLoader.cpp` and
`Processor.cpp`), using our existing Rust convolution engine. The shared loader
is specifically for cabinets, not a general reverb-IR loader.

Existing cabinet files and saved controls are not rewritten. Reloading an IR or
restarting the updated engine applies the new level handling, so saved rigs can
be substantially quieter. LEVEL remains relative output trim. Normalization is
not perceived-loudness matching or a guarantee against clipping; a bass-heavy
IR remains bass-heavy. Cabinet EQ, gate settings, NAM gain and limiter behavior
were unchanged in that level-handling update. Limiter metering and optional
cabinet cuts are described below; gate noise-floor measurement remains follow-up
work.

Verification: 144 engine-library tests and 18 relevant integration tests pass
(one existing library test remains ignored). New regressions cover 44.1/48/96 kHz
source files, 48/96 kHz engines, hot IRs whose peaks remain below full scale,
stereo linking, quiet-file attenuation, and unchanged frequency balance. An
offline impulse render through the user's `s-pres8.wav` produced the expected
0.01584893 kernel energy at 48 kHz and retained its 10 dB emphasis at 120 Hz
relative to 1 kHz. Physical guitar listening remains to be checked.

## Limiter visibility (November 12, 2026)

The live meter strip shows separate RIG and OUT gain-reduction readings, in
positive dB of attenuation. RIG measures the pedalboard safety stage; OUT measures
the final monitor mix, including backing tracks and the metronome. Patch Lab
replaces the pedalboard route, so RIG is marked inactive while a patch is active.
The two readings are independent interval peaks and are not summed.

Each reading captures the greatest actual smoothed reduction in the polling
interval, including release. Metering adds one atomic minimum-gain update per
stage per block; conversion to dB happens on the control thread. No limiter
thresholds, timing, audio samples, or buffering are changed. Missing fields from
older engines and stopped/disconnected audio display unavailable readings.

Sustained reduction can explain a flattened attack or compressed feel. Lower
upstream output levels when it is unwanted; this meter does not diagnose the
source of hiss or change the gate threshold.

Verification: 183 engine tests pass (one existing library test remains ignored),
including known attenuation, limiter release, linked stereo gain, independent
stage intervals, protocol fields, and live pedalboard/Patch Lab route switching.
Existing cabinet and live/offline consistency tests pass. All 37 focused and
existing Flutter widget tests pass, including 320-pixel width with larger text,
stale-value clearing, and the full 480×320 rig shell. Physical guitar listening
remains a user check; the running audio session was not restarted.

## Flat cabinet defaults (November 12, 2026)

New pedalboard rigs use cabinet LEVEL 0 dB, LOW CUT OFF and HIGH CUT OFF. The
cabinet remains fully wet when enabled. This matches TONE3000's neutral cabinet
control defaults: its EQ starts as zero-gain shelves/bells, not active high/low
pass filters. Its six-band EQ can be changed to apply cuts deliberately.
References: [block controls](https://github.com/tone-3000/tone3000-plugin/blob/main/plugin/include/ChainBlock.h),
[EQ defaults](https://github.com/tone-3000/tone3000-plugin/blob/main/plugin/src/BlockEq.cpp).

The existing cutoff fields use 0 Hz internally to mean OFF; UI controls show
OFF, not 0 Hz. Positive frequencies retain their original ranges: low cut
20–500 Hz and high cut 1–20 kHz. Saved positive cutoffs remain active, so existing
presets keep their intended filtering. New and missing-field defaults are flat.
On/off transitions are smoothed; fully bypassed cuts leave the convolved signal
unchanged. The fixed −18 dB cabinet pad and automatic energy normalization remain
in effect independently of the user-facing LEVEL control.

These are control-default changes, not a claim of bit-identical TONE3000 output:
our existing filter design, IR preparation, and stereo normalization policy
remain our own. TONE3000 also provides input gain, dry/wet mix, a six-band EQ
with frequency/gain/Q and pre/post placement, and stereo/mono selection for
stereo IR files. Those additional controls are not introduced by this change.

Verification: 189 engine tests pass (one existing library test remains ignored).
Coverage includes exact flat impulse response, independent low/high attenuation,
unequal stereo-channel balance, smooth cut bypass transitions, valid control
ranges, legacy preset retention, and live/offline render consistency.
The 39 cabinet and existing app widget tests pass, including the loaded cabinet
at 480×320, its scrollable vertical controls, and local cutoff recall when
toggling OFF/ON. Saved OFF is represented by zero; local recall of the previous
frequency lasts for the lifetime of that control view, not across app restarts.

## Remaining rollout

Drive NAM and NAM Head now support per-slot metadata-based normalization and
matching ±24 dB gain ranges. New rigs enable normalization; old saved rigs without
the flags keep it off. Neutral gain, fully wet mix, and flat EQ were already in
place. See [NAM levels](nam-levels.md) for reference behavior, calibration rules,
and the intentional distinction from Patch Lab nodes.

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
