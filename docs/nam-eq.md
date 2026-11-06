# Per-NAM six-band EQ

Open **Drive NAM** or **NAM Head**, then select **6-BAND EQ**. Each slot has an
independent EQ; the existing trim controls and standalone EQ pedal are unchanged.

- Six bands: low shelf/cut, four bells, high shelf/cut.
- Frequency: 20 Hz–20 kHz (logarithmic slider); gain: ±15 dB; Q: 0.1–10.
  Higher Q is narrower for bells and more resonant for shelves/cuts.
- The first/last band can become a 12 dB/octave low/high cut. Gain is hidden and
  ignored for cuts, but retained when switching back to a shelf.
- **PRE NAM** shapes the input driving the nonlinear model. **POST NAM** shapes
  the wet output. On Drive, both placements leave the parallel dry mix untouched.
- EQ bypass retains settings. Reset restores flat defaults and POST placement.
- Settings belong to presets and persist offline. Missing legacy fields default
  to flat EQ, preserving existing sound. These filters do not expose or change
  the original captured hardware's physical tone knobs.

Default band frequencies are 100, 250, 650, 1600, 3500 and 8000 Hz; gains are 0 dB;
Q is 0.71, 1, 1, 1, 1.4 and 0.71 respectively. EQ starts enabled but exactly flat.
Boosts can increase distortion and output level: reduce gain or rig level if the
existing safety limiter begins working. No automatic makeup gain is applied.

## Reference and implementation

Behavioral reference: TONE3000's six-band channel-strip roles, defaults, bypass,
pre/post placement and wet-only routing in
[BlockEq.h](https://github.com/tone-3000/tone3000-plugin/blob/ab5e5ae4388753184687d67816815aa481f2cea9/plugin/include/BlockEq.h)
and [BlockEq.cpp](https://github.com/tone-3000/tone3000-plugin/blob/ab5e5ae4388753184687d67816815aa481f2cea9/plugin/src/BlockEq.cpp).
The filters are an original Rust implementation of the standard
[RBJ cookbook equations](https://www.w3.org/TR/audio-eq-cookbook/); no JUCE/TONE3000
source is imported. The UI reuses AERO's vertical chrome control bank.

Unlike the reference's shared chain mutex and instantaneous coefficient changes,
AERO compiles coefficients on the control/loading thread and publishes a fixed
atomic bank. The existing engine-state mutex serializes writers. The callback
attempts one coherent snapshot; if publication overlaps, it keeps its previous
bank until the next block. It never waits, allocates, or computes trigonometric
filter coefficients. Coefficients/history use double precision internally.

Separate pre/post filter histories transition toward their new targets over
10 ms, including enable, cut type, band edits and routing changes. During a
position transition both locations briefly contribute; this is not crossfading
two complete NAM rigs. Flat, settled filters skip processing. No audio buffering
or lookahead is added (IIR filters still have frequency-dependent phase).

The EQ wrapper sits inside each slot's trim/mix and bypass wrappers, before
cabinet processing. Head EQ is before the existing bass/mid/treble finishing
stage. Stereo cabinet/effect processing downstream remains unchanged. The common
rig graph is used for monitoring, wet recording, preview and reamp.

## Verification

- Measured shelf, bell and cut responses at 44.1, 48 and 96 kHz.
- Exact flat/disabled identity; unmodified dry mix; distinct pre/post nonlinear
  output; smooth finite transitions; callback-size invariance; extreme/bad-value
  defense; coherent publication while the control thread updates.
- API bounds, preset dirty state, snapshot fields, legacy defaults, preset and
  offline-state round trips.
- Live graph versus offline render with actual NAM models, both EQ placements,
  and different callback sizes.
- Touchscreen controls at 480×320 and 800×600, logarithmic frequency endpoints,
  mode switching, saved-state restoration and failed-command rollback.
- 109 engine tests and all 61 Flutter tests pass; all-target Clippy and Flutter
  analysis are clean. The separate manual benchmark also passes.

Manual CPU check (isolated EQ, not a complete NAM rig or interface latency):

```sh
cargo test --release -p pedal-engine --lib nam_eq_processing_benchmark -- --ignored --nocapture
```

On the i9-13900H development laptop, one release run processed ten seconds of
48 kHz audio in 1.98 ms flat or 35.37 ms with all six bands active. The worst
64-frame active block took 41.95 µs against a 1.33 ms deadline; no deadlines
were missed. This is a microbenchmark, not a whole-app or Pi guarantee.

Physical guitar listening and Raspberry Pi performance remain to be verified.
