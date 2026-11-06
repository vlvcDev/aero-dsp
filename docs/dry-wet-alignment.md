# Confidence-checked Drive NAM alignment

Open **Drive NAM → TONE → AUTO DRY/WET ALIGN**. It is enabled by default,
including legacy presets, and can be disabled for the original dry blend.
The switch is stored with the preset and offline engine state. The status shows
an accepted delay in samples/ms, an already-aligned capture, or an unchanged
uncertain/inverted capture.

This addresses cancellation/comb filtering when Drive MIX blends a delayed NAM
capture with the dry input. It delays only the dry share to meet the wet signal;
it does not advance the model or remove its inherent delay. Fully wet playback
is sample-identical, fully dry stays undelayed, and pedal bypass stays unchanged.
Near the fully-dry endpoint (the final 0.1% of mix), the dry delay is faded out.

## Measurement

Behavioral reference:
[TONE3000 model-latency research](https://github.com/tone-3000/tone3000-plugin/blob/ab5e5ae4388753184687d67816815aa481f2cea9/plugin/docs/model-latency.md).
This is an original Rust implementation of a linear sweep and normalized
time-domain correlation; no TONE3000 code is imported. We use its sweep/peak and
nearby-lobe checks as the reference, with additional conservative gates.

- Probe only Drive captures, off the audio callback, at their actual sample rate
  and chosen Lite/Full tier. Amp-only loading does not do this extra work.
- Use a separate analysis model: 100 ms silence, a 100 ms linear 30 Hz–20 kHz
  sweep (upper limit 45% of sample rate), and 10 ms trailing silence. Sweep edges
  have 5 ms fades. Repeat at amplitudes 0.03 and 0.1.
- Remove output DC, then search nonnegative delays up to 10 ms. Require
  normalized correlation ≥0.35, distant peak ratio ≥2, nearby lobe ratio ≥1.2,
  normal polarity, and exactly matching integer lags at both probe levels.
- Reject silence, nonfinite/failed processing, boundary peaks, competing lobes,
  inverted polarity and level-dependent results. A failed measurement does not
  prevent a valid model from loading; no correction is applied.
- Destroy the analysis instance and load a pristine playback instance. Probe
  audio is never sent to the device or left in the live model's history.

The result belongs to that prepared model instance. Restart, preset/capture
loads and live-tier changes remeasure locally, including offline. Reamps use
their selected render tier's own measurement. Measurement runs even if the
switch is off so it can be enabled immediately. This costs additional model
loading/analysis time; it is not a continuous real-time estimator.

## Audio path and limitations

A preallocated 10 ms dry ring runs alongside the wet model. Delay changes and
enable/disable use a 10 ms crossfade between read taps, without varying read
speed/pitch. Metadata is queried after processing, so a model swap uses the new
capture's result. There is no new callback allocation, waiting or correlation.
The same graph handles monitoring, wet recording, previews and reamps.

This version uses integer delays and deliberately does not flip polarity. It
does not compensate frequency-dependent cabinet/EQ phase, guarantee alignment
at every distortion level, or correct interface/recording latency. NAM EQ is
excluded from measurement: its phase/color remains intentional. There is no
manual offset or fractional-delay interpolation in this stage.

The transient `pedal_alignment` status is not saved or trusted from disk.
Only `pedal_align_enabled` persists. If the file has no trustworthy result,
the original blend remains available rather than guessing.

## Verification

- Synthetic 0/1/17/37/127-sample delays with gain, DC and nonlinear distortion;
  repeatability across block sizes; rejection of silence, inverted polarity,
  ambiguous lobes, boundary peaks, level-dependent delay and processing failure.
- Actual NAM Lite/Full loads: analysis leaves pristine playback sample-identical.
- Aligned 50% blends null against delayed wet; 0%/100%, disabled, uncertain and
  bypass cases preserve their expected output. Tap changes, model swaps and
  discontinuity clearing are tested.
- A generated 37-sample Linear NAM fixture verifies the complete loader → live
  graph → renderer path and restores a signal otherwise cancelled by dry blending.
- API validation, preset persistence, offline switch restoration, transient
  measurement invalidation and 480×320 UI failure rollback are covered.

Physical guitar listening and Raspberry Pi load-time/performance testing remain
manual follow-ups. A correlation score is a diagnostic, not a probability that
the tone will sound better.

Validation: 115 engine tests, 10 bridge tests and all 63 Flutter tests pass.
Engine/bridge all-target Clippy, Flutter analysis and Rust format checks pass.
