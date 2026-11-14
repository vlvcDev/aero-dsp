# Control protocol v1

## Patch Lab audio

- `patch_status`: reports `available`, `active`, `faulted`, `switching`; when
  available, includes device `sample_rate`, global `bypassed`, and selected patch
  `latency_samples` (nominal added algorithmic latency).
- `apply_patch`: payload contains version-1 `document` and absolute asset
  `directory`. Prepares off the audio thread and queues a faded route exchange.
  Requires matching device rate and at most 64 samples of added algorithmic latency. A successful
  `{queued:true}` response means accepted, not yet audible; poll status.
- `deactivate_patch`: queues return to the existing pedalboard.
- `render_patch`: same payload, plus optional absolute WAV `path` (default:
  bundled test clip). Renders up to 30 seconds plus 3-second tail; response
  includes `path`, `duration_seconds`, and audition `attenuation_db`.

Success responses use type `patch`; failures use `error` / `patch_failed`.
Meters include `patch_active` so clients can show the live route outside the
editor. Preparation/rendering should use a separate connection from meters and
bypass. Activation is session-only and resets on audio-worker restart. Relative
NAM/IR paths must resolve within `directory`, including symlink containment.

Messages are UTF-8 JSON objects separated by newlines. Every request has `api`, `id`, `type`, and `payload`. Responses echo `id`; unsolicited events use a new engine-generated ID.

## Per-NAM EQ controls

`set_control` accepts `pedal_eq_` or `amp_eq_` prefixes, followed by:

- `enabled`, `pre`: 0 or 1; `pre=0` is wet-only post-model EQ.
- `reset`: 1 restores flat defaults, enabled and post-model; 0 does nothing.
- `band_N_freq_hz`: 20–20000 Hz, `band_N_gain_db`: −15–15 dB,
  `band_N_q`: 0.1–10, where N is 0–5.
- `band_N_cut`: 0 shelf / 1 cut, valid only for N=0 or N=5.

Each state snapshot includes `pedal_eq` and `amp_eq` objects containing
`enabled`, `pre`, and six `bands` (`freq_hz`, `gain_db`, `q`, `cut`). These values
persist in presets and offline state. Unknown fields/indices, nonfinite values,
and out-of-range controls are rejected without changing settings. Legacy
presets default to flat EQ. See [NAM EQ](nam-eq.md) for routing details.

## Drive dry/wet alignment

`set_control` with `control: "pedal_align_enabled"` accepts only 0 or 1
(default 1). This is a preset control. State snapshots also include transient
`pedal_alignment`: null when unmeasured, otherwise `{status, delay_samples,
correlation, peak_ratio, lobe_ratio}`. Status is `accepted`, `uncertain`, `silent`,
`inverted`, `at_limit`, `level_dependent` or `probe_failed`. Only `accepted`
results affect the dry blend; a zero accepted delay requires no correction.
Scores are diagnostics, not probabilities. Measurements are recalculated on
model load and are not serialized into saved engine state or presets.

See [dry/wet alignment](dry-wet-alignment.md) for scope and validation.

## Get state

```json
{"api":1,"id":"ui-1","type":"get_state","payload":{}}
```

```json
{"api":1,"id":"ui-1","type":"state","payload":{"engine":"ready","bypassed":false,"pre_bypassed":false,"amp_bypassed":false,"preset":"Default","preset_id":null,"preset_dirty":false,"pre_model":null,"model":null,"gate_enabled":false,"chorus_enabled":false,"delay_enabled":false,"reverb_enabled":false,"looper_mode":"stopped","tuner_enabled":false,"sample_rate":48000,"buffer_frames":64}}
```

## Set bypass

```json
{"api":1,"id":"ui-2","type":"set_bypass","payload":{"bypassed":true}}
```

The response is a fresh `state` snapshot. Invalid input returns an `error` message whose payload includes `code` and `message`. Unknown fields are ignored so compatible fields can be added within protocol v1.

Authoritative state changes are persisted by the engine outside the audio callback. On restart, the daemon restores the last complete state from `/var/lib/pedal/engine-state.json`; this operation has no network or catalog-service dependency.

## Select a prepared local model

```json
{"api":1,"id":"ui-3","type":"select_model","payload":{"path":"/var/lib/pedal/models/clean-combo.nam","preset":"Clean Combo","slot":"amp"}}
```

`slot` is `pre` for a pedal capture or `amp` for the main amp/full-rig capture; omission defaults to `amp` for compatibility. The engine requires an existing absolute `.nam` path. It validates and loads the model on the requesting control thread, then stages the prepared model in that slot's single-slot mailbox. The audio thread uses a non-blocking mailbox check and swaps ownership at the next block boundary. The previous model is retained for disposal by a non-audio thread. Invalid models leave both active models and persisted state unchanged.

The state response includes `pre_model` and `model` (the amp slot). At startup, both are restored locally and processed in this fixed order:

```text
input -> gate -> compressor -> pre/pedal NAM -> amp NAM -> EQ -> chorus -> delay -> reverb -> looper -> output
```

## Control or clear one slot

Each model slot can be bypassed independently without unloading its model:

```json
{"api":1,"id":"ui-4","type":"set_slot_bypass","payload":{"slot":"pre","bypassed":true}}
```

Set `slot` to `pre` or `amp`. The engine changes an atomic flag used directly by the audio thread and persists the choice for offline restart. Selecting a new model automatically engages that slot.

A slot can also be returned to clean passthrough:

```json
{"api":1,"id":"ui-5","type":"clear_model","payload":{"slot":"amp"}}
```

Clearing is staged through the same non-blocking mailbox as model selection. It removes only the requested slot, resets its slot bypass, and leaves the other model running. Both commands return a fresh `state` snapshot.

## Set an adjustable control

```json
{"api":1,"id":"ui-6","type":"set_control","payload":{"control":"amp_bass_db","value":3.5}}
```

The supported controls and ranges are:

| Control | Range | DSP position |
|---|---:|---|
| `pedal_drive_db` | -24 to +24 dB | Before the pedal NAM |
| `pedal_mix` | 0.0 to 1.0 | Pedal dry/wet blend |
| `pedal_level_db` | -24 to +24 dB | After the pedal blend |
| `amp_bass_db` | -12 to +12 dB | After the amp NAM |
| `amp_mid_db` | -12 to +12 dB | After the amp NAM |
| `amp_treble_db` | -12 to +12 dB | After the amp NAM |
| `amp_tight_hz` | 0 (OFF), or 20 to 300 Hz | TIGHT: 12 dB/oct high-pass on the amp NAM's input, after the calibrated hand-off. Trims lows before the gain stage so high-gain palm mutes stay tight (100–150 Hz is a classic tight-boost corner). Saved with the preset; glides and crossfades click-free; default OFF |
| `amp_volume_db` | -24 to +24 dB | After the amp tone stack |
| `pedal_normalize_enabled` | 0 off, 1 on | Metadata-based Drive wet-output normalization; calibrated handoff takes priority |
| `amp_normalize_enabled` | 0 off, 1 on | Metadata-based Head output normalization |
| `cabinet_gain_db` | -24 to +12 dB | Cabinet output trim; default 0 dB |
| `cabinet_low_cut_hz` | 0 (OFF), or 20 to 500 Hz | Optional post-convolution 12 dB/oct Butterworth low cut (80–100 Hz tightens high gain); knob changes glide over ~20 ms; default OFF |
| `cabinet_high_cut_hz` | 0 (OFF), or 1000 to 20000 Hz | Optional post-convolution 12 dB/oct Butterworth high cut; knob changes glide over ~20 ms; default OFF |
| `gate_threshold_db` | -120 to -10 dB | Noise gate open threshold, measured on the clean input. Goes down to -120 dB because high-gain captures make notes fading through -90 dB clean still loud |
| `squelch_enabled` | 0 off, 1 on | Device setting: feeds the models exact zeros while the clean input sits at the interface noise floor (NAM is deterministic, so zero in is silence out). Applies to the rig and Patch Lab; DI takes and bypass stay raw. Default on |
| `squelch_threshold_db` | -130 to -60 dB | Device setting: squelch open threshold on the clean input (closes 3 dB lower after a 50 ms hold). Set with noise-floor LEARN; default -105 |
| `gate_hysteresis_db` | 0 to 30 dB | Once open, the gate stays open until the clean input falls this far below the threshold; default 12 dB (was a fixed 6 dB). Raise it so sustained notes and pinch harmonics ring out under a high threshold |
| `nam_quality` | 0 Lite, 1 Full | Live NAM tier preference; prepares both slots before a faded swap |
| `render_nam_quality` | 0 Lite, 1 Full, 2 Same as live | Offline NAM tier override; does not alter the live rig |
| `nam_oversampling` | 1 (off), 2, 4, 8 | Live NAM oversampling: phase-interleaved native-rate instances behind a half-band oversampler (Tone3000 design). Cuts high-gain aliasing; total NAM CPU scales with the factor, but phases run in parallel on the DSP worker pool (one worker per spare core, max 7; set `PEDAL_DSP_WORKERS=0` to run serially), so audio-thread time per block stays close to 1x. LSTM captures stay at 1x. Prepares both slots before a faded swap; default 1 |
| `render_nam_oversampling` | 0 Same as live, 1, 2, 4, 8 | Offline render/preview override; does not alter the live rig; default 0 |
| `gate_hold_ms` | 0 to 500 ms | Consecutive time below the closing threshold before release |
| `gate_release_ms` | 5 to 1000 ms | Gate gain-release time constant |
| `gate_range_db` | 0 to 90 dB | Maximum closed-gate attenuation; 0 leaves unity gain |
| `dehiss_threshold_db` | -100 to -30 dB | DEHISS: high-band level below which the band above `dehiss_frequency_hz` is reduced (2 dB per dB below). Set with LEARN; default -70 |
| `dehiss_frequency_hz` | 1500 to 8000 Hz | DEHISS crossover: only content above it is reduced, so decaying notes keep their body; default 3000 |
| `dehiss_range_db` | 0 to 40 dB | DEHISS maximum high-band reduction; default 24 |
| `compressor_threshold_db` | -48 to 0 dB | Compressor threshold |
| `compressor_ratio` | 1 to 20 | Compressor ratio |
| `compressor_attack_ms` | 0.5 to 50 ms | Gain-reduction attack: 10-30 ms lets the pick transient through (articulate); ~1 ms flattens it (sustain). Default 10 |
| `compressor_release_ms` | 30 to 500 ms | Gain-reduction release; default 120 |
| `compressor_mix` | 0 to 1 | Parallel blend with the dry input (zero latency, no comb filtering); default 1 |
| `compressor_level_db` | -12 to +24 dB | Manual makeup, added to automatic makeup; default 0 |
| `compressor_auto_makeup` | 0 off, 1 on | Adds back the reduction a -12 dBFS pick would get, so engaging the compressor does not change how hard the amp is driven; default on |

The compressor's detector ignores content below ~100 Hz (12 dB/oct) so low strings do not over-trigger it; the audio path stays full-range. `meters` reports `compressor_reduction_db`, the peak gain reduction since the previous meter request.
| `eq_low_db`, `eq_mid_db`, `eq_high_db` | -12 to +12 dB | Post-amp three-band EQ |
| `chorus_rate_hz` | 0.05 to 8 Hz | Chorus LFO speed |
| `chorus_depth`, `chorus_mix` | 0.0 to 1.0 | Chorus depth and blend |
| `delay_time_ms` | 20 to 1800 ms | Delay time |
| `delay_feedback`, `delay_mix` | 0.0 to 0.92 / 1.0 | Delay repeats and blend |
| `reverb_decay_seconds` | 0.3 to 12 s | Reverb decay |
| `reverb_mix` | 0.0 to 1.0 | Reverb blend |

The response contains the updated state. Invalid names, non-finite values, and out-of-range values are rejected without changing state. The engine converts dB to linear gain on the control thread, publishes the fixed-size values atomically, and ramps changes across an audio block to avoid a hard discontinuity. Controls persist with the rig for offline restart.

## Switch an effect or operate the looper

Each built-in effect is independently bypassable. `effect` is one of `gate`, `compressor`, `eq`, `chorus`, `delay`, or `reverb`.

```json
{"api":1,"id":"ui-7","type":"set_effect_bypass","payload":{"effect":"delay","bypassed":false}}
{"api":1,"id":"ui-8","type":"looper_action","payload":{"action":"record","bpm":100,"bars":4}}
```

Looper actions are `record`, `play`, `overdub`, and `stop`. `record` starts a four-beat audio-thread count-in, then records on beat one for the requested 1, 2, 4, or 8 bars (30–300 BPM). It automatically changes to playback on the selected bar boundary. Configurations longer than 30 seconds are rejected. The looper keeps up to 30 seconds of mono post-reverb audio in RAM. Its audio is intentionally not persisted: after a restart it reports `stopped` and begins with an empty loop.

## Open the clean-input tuner

```json
{"api":1,"id":"ui-9","type":"set_tuner_enabled","payload":{"enabled":true}}
```

The tuner receives the input after the protective 30 Hz high-pass but before the gate, NAM models, effects, and global bypass. It does not mute or otherwise alter the guitar signal. Clients should disable it after closing their tuner view; the engine also disables it when restarted.

## Presets

A preset stores both model paths, both slot bypass states, each effect's on/off state, and every adjustable control. Hardware negotiation, the global emergency bypass, and in-memory looper audio are not part of a preset.

```json
{"api":1,"id":"ui-8","type":"save_preset","payload":{"name":"Crunch"}}
{"api":1,"id":"ui-9","type":"list_presets","payload":{}}
{"api":1,"id":"ui-10","type":"load_preset","payload":{"id":"preset-1"}}
{"api":1,"id":"ui-11","type":"rename_preset","payload":{"id":"preset-1","name":"Lead"}}
{"api":1,"id":"ui-12","type":"delete_preset","payload":{"id":"preset-1"}}
```

`list_presets` returns a `presets` response containing stable IDs, names, and pedal/amp model paths. The other operations return authoritative engine state. Names are trimmed and limited to 64 characters. Loading prepares every referenced NAM model before staging either slot; a missing or invalid file leaves the current rig unchanged. `preset_dirty` becomes true when a saved rig's slot or control settings change, and resets after loading or saving a preset.

The library is atomically persisted to `/var/lib/pedal/presets.json`. Deleting the active preset removes it from the library but deliberately leaves the current sound loaded.

## Meter snapshot

```json
{"api":1,"id":"ui-7","type":"get_meters","payload":{}}
```

```json
{"api":1,"id":"ui-7","type":"meters","payload":{"input_db":-18.4,"output_db":-20.1,"clipped":false,"rig_limiter_reduction_db":0.0,"output_limiter_reduction_db":0.0,"cpu_percent":3.2,"xruns":0,"tuner_hz":82.4,"tuner_confidence":0.93}}
```

Meters are display telemetry, not authoritative state. The Flutter client polls at 10 Hz, below the 20 Hz budget. Peaks and the clip flag cover the interval since the previous meter request; `xruns` is cumulative for the current engine process. `audio_realtime_priority` is the audio thread's SCHED_FIFO priority and `dsp_worker_realtime_priority` the DSP workers' (one step below it); 0 means normal, non-realtime scheduling. `tuner_hz` and `tuner_confidence` are zero while the tuner is disabled or has no stable clean-input pitch. Clients must tolerate missing snapshots and reconnect normally.

`rig_limiter_reduction_db` and `output_limiter_reduction_db` report positive
attenuation in dB: zero means no reduction, 6 means approximately half amplitude.
Each is the greatest actual gain reduction since the preceding meter request,
including the limiter's release, rather than an input/output peak difference.
The rig stage protects the pedalboard at −1 dBFS; the output stage protects the
final monitor mix at 0 dBFS, including timeline playback and metronome. These are
independent interval peaks and must not be added to estimate total reduction.
Patch Lab replaces the pedalboard route, so its audio reaches the output stage
but not the pedalboard's rig limiter; the UI marks RIG inactive when `patch_active`
is true. No processed blocks in an interval produces zero reduction. Older
engines omit these fields; clients display unavailable rather than assuming
zero, and clear the readings when audio is stopped or disconnected.

## Render a hardware-free preview

```json
{"api":1,"id":"ui-12","type":"render_preview","payload":{"source":"built_in","processed":true}}
```

```json
{"api":1,"id":"ui-12","type":"preview","payload":{"path":"/tmp/pedal-preview.wav","duration_seconds":6.19}}
```

`source` may be `synthetic`, `built_in`, or `file`. A file source also supplies an absolute `path` to a WAV chosen by the local user. `processed` defaults to `true`; when false, the engine renders the resampled dry input for A/B listening. WAV inputs may be mono or multichannel 16/24/32-bit PCM or 32-bit float and are limited to the first 60 seconds. The built-in source is configured with `PEDAL_TEST_AUDIO_FILE`.

For processed playback, the engine renders offline through a newly prepared copy of the current pedal NAM, pedal controls, amp NAM, amp tone stack, bypass state, and safety stage. It writes a 48 kHz, 16-bit stereo WAV atomically outside the real-time audio thread. The destination remains engine-controlled through `PEDAL_PREVIEW_FILE`.

This is a development and auditioning aid, not a replacement for live interface testing. The Flutter Linux client plays the returned file through `pw-play`, falling back to `aplay`, and can stop the player without interrupting the engine.
The client allows up to two minutes for an offline render so unoptimized development builds and dual-NAM rigs can complete without a false timeout.

## Dehiss

DEHISS is an optional pedal (add it with `set_pedal_visible` and
`"pedal":"dehiss"`; switch it with `set_effect_bypass` and `"effect":"dehiss"`): a
downward expander on the amp/cab output's high band only, applied before the
gate and any modulation, delay or reverb. Stereo channels share one detector.
It complements the gate: lower the gate threshold for sustain and let DEHISS
remove the hiss that rides over decaying notes.

LEARN sets the threshold from the idle hiss floor in two non-blocking steps:

```json
{"api":1,"id":"ui-20","type":"dehiss_learn","payload":{"phase":"start"}}
{"api":1,"id":"ui-21","type":"dehiss_learn","payload":{"phase":"finish"}}
```

Between them the player stays silent for about 2 s. `start` returns the
current state; `finish` sets `dehiss_threshold_db` 10 dB above the loudest
measured high-band hiss envelope, saves, and returns the new state, or
`dehiss_learn_failed` if no audio was measured. Measuring works while DEHISS
is off and never alters the audio.

## Noise-floor LEARN

High-gain captures apply 60-100 dB of gain to near-silent input, so an
interface's own converter floor becomes loud hiss even with every knob at
zero. Noise-floor LEARN measures that floor and sets the input squelch 6 dB
above it, and the gate threshold so its close point sits 4 dB above it:

```json
{"api":1,"id":"ui-30","type":"noise_floor_learn","payload":{"phase":"start"}}
{"api":1,"id":"ui-31","type":"noise_floor_learn","payload":{"phase":"finish"}}
```

Keep the guitar plugged in with its volume up and do not touch the strings
for ~2 s between the steps. `finish` returns the updated state (squelch is a
device setting; the gate threshold marks the preset dirty), or
`noise_floor_learn_failed` if no input was measured.

