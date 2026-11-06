# Audio quality rollout

## Completed: fidelity foundation

- Fixed the fractional-delay interpolation direction shared by chorus and flanger.
- Replaced sample-threshold/double gating with a clean-input envelope, hysteresis,
  hold, and a time-aligned post-model gain. Gate timing is independent of block size.
- Replaced sample hard clipping as the normal output protection with a zero-lookahead
  peak gain limiter and an 80 ms release. A final clamp remains as a safety net.
- Moved the existing 30 Hz DC rejection after rig processing, so model-generated
  offsets are rejected and new DI recordings are not prefiltered. Nonfinite values
  are contained rather than poisoning the protection filter.
- Consolidated WAV decoding and offline sample-rate conversion. Rubato's filtered
  FFT conversion replaces linear interpolation for cabinet import and previews.
  Conversion compensates reported latency and returns the rounded frame count.
- Unprocessed previews/trims preserve source channels. New previews and guitar
  takes are float32 WAV masters, retaining precision and headroom; existing integer
  PCM takes remain readable. Source files are not rewritten.
- Waveform extraction uses peaks across channels, avoiding stereo phase cancellation.
- Stable Audio reference-duration validation also handles float WAV headers.

The new conversion/file-writing helpers allocate and perform I/O; they belong only
on control/offline paths, never in live processing. No live NAM operating rate changed.
Float masters can exceed 0 dBFS; a future master bus/export stage must handle playback
headroom consistently. A float file alone does not prevent DAC clipping.

## Completed: cabinet, effects, and reliability pass

- Existing presets use the upgraded algorithms; no legacy mode or preset deletion.
- Zero-added-latency partitioned convolution retains 200 ms of a mono cabinet IR
  by default, versus the previous 1,024 taps. Import preserves onset and tapers
  truncated ends over 5 ms. Prepared IR swaps fade down/up without running two IRs.
- RBJ shelving/peaking EQ replaces the first-order split bands. Coefficients and
  gain changes are smoothed over time; drive/mix/level no longer ramp per callback.
- Compressor uses an RMS detector, a soft knee, and attack/release gain smoothing.
- Reverb uses four input diffusers and an eight-line, damped, modulated feedback
  network with decay-calibrated gains instead of four independent comb filters.
- NAM swaps use a short fade-down/swap/fade-up. NAM/global bypass crossfades over
  10 ms, then stops the bypassed graph to save CPU. Frozen-state/tail resumption
  during long bypass is not yet addressed; these are not spillover crossfades.
- Paired clean/wet capture blocks use a bounded queue and background ring writer.
  Saving/inspecting the ring cannot lock the audio thread. Queue saturation is
  counted and represented by silence, including trailing gaps; elapsed time is
  not silently shortened. Track capture warns about session-wide lost frames.
  The queue is finite (2,048 packets of up to 128 frames), not a lossless guarantee.
- Pitch autocorrelation runs on a background thread with a bounded latest-window
  queue. The clean-input tap only copies snapshots. Analysis removes window DC;
  peak refinement uses the selected fundamental rather than a competing harmonic.
- Audio-worker failure exposes a recovering status and retries with capped backoff;
  an in-memory looper is marked stopped after failure. Reconnection rebuilds DSP.
- Meter UI distinguishes DSP deadline usage from total CPU, reports cumulative
  worst usage/missed deadlines, and displays recovery errors in its tooltip.
- NAM bridge exposes optional input/output calibration and loudness metadata.
  The calibration pass below now uses the physical level metadata when available.
- Compact header scrolls horizontally so all controls remain reachable at 480px.

## Verification

- 95 Rust workspace tests, including NAM bridge and engine integration coverage.
- 28 Flutter tests and Flutter analysis pass, including native timeline recording.
- 38 catalog tests and 7 development-tool tests.
- Workspace/all-target Clippy with warnings denied passes. Audio-worker arguments
  are grouped into a context rather than suppressing the previous lint findings.
- Isolated real-engine IPC preview test: stereo float output decoded with FFmpeg;
  frame count, rate, stereo polarity, and non-silence verified.
- Added regressions for gate block-size invariance, fractional-delay continuity,
  stop-band alias rejection, resampling alignment/length, output DC/nonfinite handling,
  limiter release behavior, stereo trimming, float headroom, and legacy take decoding.
- No physical interface/Pi listening or latency test performed in this stage.
- Regressions additionally cover convolution against direct FIR at partition
  boundaries, late reflections, EQ response/transition stability, reverb decay,
  callback-independent smoothing, metadata absence, snapshot contention, queue
  overflow/timing, and health telemetry.
- Isolated missing-device test verifies repeated retries, descriptive recovery
  status, and a responsive real control socket without opening the live rig.
- `cargo run -p pedal-engine --release --example quality_soak`: on this development
  laptop, 30 s synthetic 48k/64 processing of 200 ms IR + EQ + reverb measured
  deadline usage p50 1.53%, p99 2.70%, p99.9 3.74%, max 7.75%, zero misses.
  This excludes NAM, device I/O, recording, and UI; it is not a Pi qualification
  or a round-trip latency measurement. Scheduler noise makes maxima run-dependent.

## Design checkpoint

Preset compatibility resolved: user prefers the simpler universal algorithm upgrade.

Octaver decision resolved: both modes are implemented. Analog remains the default;
Clean uses Signalsmith Stretch 0.1.3, shifting down one octave before NAM processing.
The existing mix slider applies to either voice. Selection persists in presets and
offline state. Clean's octave voice has 4,096 frames / 85.33 ms algorithmic latency
at 48 kHz; the dry path remains immediate. The latency label comes from the shared
configuration, checked against library latency getters. This is not total device
latency. Switching has a brief warm-up and fade; inactive clean mode skips spectral
processing. Gate hold extends for the delayed voice to avoid cutting off its end.

The clean-mode release soak (IR + EQ + reverb + clean octave; no NAM/device/UI)
measured p50 1.60%, p99 12.06%, p99.9 21.26%, max 47.49% of the 48k/64 deadline,
zero misses over 30 s synthetic audio on this laptop. Spectral work is burstier than
analog. Pi performance and real-guitar listening remain unqualified. The tests
check three simultaneous shifted notes, mode switching, default/preset persistence,
latency reporting, invalid control values, and the 480×320 mode controls.

## Completed: common graph, calibration and quality controls

- Live guitar, captured wet audio, preview and reamp use one graph builder. Capture
  includes guitar output protection; monitor-only clicks remain outside the take.
  Regression tests compare live, recorded and rendered samples exactly.
- Offline renders flush delay/reverb/convolution/clean-octave tails (bounded at
  30 seconds), ending with a short fade. Reamp/trim metadata preserves original DI
  duration while zero-padding paired DI files, avoiding repeated tail growth.
  Existing timeline clip boundaries remain user-controlled; extend a clip to expose
  a longer reamped asset. Live bypass spillover is still deferred.
- Audio Output → Quality & Calibration provides interface-independent manual input
  trim, opt-in measured dBu reference, and preset-specific final rig level. A gain
  knob position is never interpreted as calibration. Device settings persist across
  restarts and new presets; legacy state defaults to calibration disabled.
- Available NAM input/output metadata determines input and interstage corrections,
  smoothed over 10 ms and bounded to ±24 dB. Missing metadata stays uncorrected;
  parallel pedal mixing disables automatic corrections rather than mixing units.
  These are not output-DAC calibration controls. Older DI reamps use the current
  reference, so users must restore their original recording reference.
- Match Audition Level measures a fixed reference performance, targets −18 dBFS
  RMS with −1 dBFS sample-peak headroom, and limits correction to −24…+6 dB.
  It changes final rig gain only, never drive or EQ. This is explicit audition
  matching, not LUFS normalization, automatic gain riding, or guaranteed perceived
  equality on every guitar. Analysis refuses to apply if the rig changed meanwhile.
- Portable retains 64 ms of IR, High 200 ms, Offline up to 1 s. Live supports
  Portable/High; render supports either, Offline, or Same as live (default).
  Quality does not change device buffers, sample rate, NAM precision or oversampling.
  Cabinet changes are prepared off the audio thread and use the existing fade swap.

## Completed: native timeline and stereo pass

- Timeline clips now share the guitar engine's output/sample clock rather than
  launching independent desktop players. Gain/pan/mute/solo update without
  restarting playback; looping wraps at exact frame boundaries. UI position is
  reported by the engine, not inferred from wall-clock time.
- Native timeline Record/Stop adds stereo wet + mono DI takes to a chosen guitar
  track, with an engine-timed count-in. Click and backing audio are monitor-only.
  Retrospective capture remains separate. Record-button long press exposes manual
  measured round-trip compensation in milliseconds; default zero, never guessed.
- Stereo cabinet WAVs retain independent left/right convolution and shared IR
  normalization. Post-head EQ, modulation, delay, reverb and looping retain two
  channels. NAM/gate/compression/octaver remain mono before the cabinet. Output
  protection links gain across channels to avoid image shifts.
- Stereo wet capture, file trimming and offline renders retain the channel image.
  Audition matching measures both channels without phase-cancelling them first.
- Underruns stop the timeline; a completed portion of an interrupted recording is
  retained in process memory for saving. Failed file exports remain retryable.
- See [native timeline and stereo](native-timeline-and-stereo.md) for controls,
  implementation boundaries, resource limits and the remaining hardware checks.

## Remaining work from the audit

- Model profiles and broader compatibility based on validated runtime support.
- Remaining modulation/delay parameter smoothing and explicit bypass/spillover policy.
- Optional true-peak limiting, gain-reduction telemetry and delivery export formats.
- Automatic loopback latency measurement, interactive clip crossfades and disk-streamed
  long sessions. Four-channel convolution matrices are not part of two-channel IR support.
- Deployment scheduling/denormal handling, broader runtime percentile metrics,
  and actual device-disconnect/reconnect qualification.
- Native-rate NAM parity, real-gear listening/latency qualification, and Pi/laptop
  release soak tests. Quality and buffer size must remain independent controls.

The completed passes are not completion of the overall audit roadmap.

## Algorithm references

- [RBJ EQ cookbook](https://www.w3.org/TR/audio-eq-cookbook/)
- [Diffusion and feedback-delay reverb](https://signalsmith-audio.co.uk/writing/2021/lets-write-a-reverb/)
- [Bounded capture queue](https://docs.rs/crossbeam-queue/0.3.14/crossbeam_queue/struct.ArrayQueue.html)
- [Signalsmith Stretch latency and pitch/time API](https://github.com/Signalsmith-Audio/signalsmith-stretch)
