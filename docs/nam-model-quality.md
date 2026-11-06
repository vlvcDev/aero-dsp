# NAM model size

Audio Quality now separates cabinet IR duration from NAM size.

- **Live NAM size:** Lite (0) or Full (1, default), for both NAM slots.
- **Render NAM size:** Lite (0), Full (1), or Same as live (2, default).
- These are device preferences, like the existing cabinet quality choices.
  They survive offline restarts and are retained when creating/loading presets.
  They are not exported as part of a preset's tone settings.
- A fixed-size capture stays unchanged under either choice. Full selects the
  largest tier present in the file; it cannot recover a missing Full capture.
- This does not change sample rate, audio buffer size, precision or oversampling.
- Reamps and processed previews use the rendering choice. Audition-level
  matching explicitly uses the live tier. Existing recorded wet audio is not
  retroactively changed; reamp it to use another tier.

## Implementation

The bridge detects `nam::SlimmableModel` with nonempty size breakpoints. It uses
0.0 and 1.0 as endpoint selections (not 0.5, which can select the upper tier).
Models are reset with the correct rate/buffer before selecting the size, then
prewarmed. For slimmable models a silent process block drains any staged child
handoff on the loading thread, not on the live callback.

Live changes prepare replacements for both slots before publishing either.
A failure leaves the setting and running models unchanged. Prepared replacements
use the existing short fade-down/swap/fade-up mechanism; this is not seamless
dual-engine crossfading. No in-place tier changes occur in the audio callback.

Implementation-order reference: [TONE3000 NamEngine at ab5e5ae](https://github.com/tone-3000/tone3000-plugin/blob/ab5e5ae4388753184687d67816815aa481f2cea9/plugin/src/NamEngine.cpp#L37).
This is an original adaptation around our existing NAM Core API, not an import
of JUCE or a copied TONE3000 implementation. Existing NAM Core licensing applies.

## Verification and measurement

Bridge tests cover A2 containers, generic slimmable containers, slimmable WaveNet,
fixed-size LSTM invariance, native-rate validation, and irregular block sizes.
Engine tests cover failed preparation rollback, API bounds, offline settings,
new-preset retention, and actual rendering with separate live/render sizes.
UI coverage includes 480×320 scrolling and rollback of failed selections.

Validation: 101 pedal-engine tests, 6 NAM bridge integration tests, and all 58
Flutter tests pass. Engine/bridge all-target Clippy, Flutter analysis, and Rust
format checks pass. Physical-interface listening and Raspberry Pi qualification
remain manual follow-ups.

On this x86_64 development machine (Intel Core i9-13900H), three alternating
release-build runs of the bundled `A2.nam` at 48 kHz / 64 frames processed ten
seconds of synthetic input as follows:

| Embedded tier | Median processing time | Slowest block across runs | Missed deadlines |
| --- | ---: | ---: | ---: |
| Lite | 129.80 ms | 99.31 µs | 0 |
| Full | 595.59 ms | 145.55 µs | 0 |

Lite used about 78% less processing time in this particular example. These are
processing measurements, not input-to-output latency; tone and savings depend
on the model's actual embedded tiers. The benchmark excludes model loading and
prewarming. It does not establish subjective equivalence between Lite and Full.

Compare a particular model on the target machine with:

```sh
cargo run --release -p pedal-engine --bin pedal-nam-soak -- /absolute/model.nam --lite
cargo run --release -p pedal-engine --bin pedal-nam-soak -- /absolute/model.nam --full
```

The tool prints whether multiple sizes are supported, total processing time,
worst block and missed deadlines. This is synthetic single-model processing,
not physical interface latency or a full pedalboard/Pi qualification.
