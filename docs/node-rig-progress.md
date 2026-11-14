# Node rig implementation

## First slice — experimental routing core

`node_rig` implements a versioned, serializable graph and an independently tested
stereo execution prototype. Supported nodes are Input, Output, Gain, Mixer and
intentional sample Delay. Fan-out supplies splitting without a special processor.
Preparation validates IDs, connections, arity, gain bounds, cycles and reachability,
then creates a topological execution order with bounded delay storage.

Processing is in-place, maintains state across arbitrary block sizes and contains
nonfinite samples. It has no heap operations in its processing loop. Mixing is an
explicit unnormalized sum; this prototype does not replace final output protection.
Limits: 64 nodes, eight inputs per mixer and 192,000 delay frames across the graph.
Delay lengths are samples, not rate-independent durations.

This is not yet the live engine, an editor, or a new amp model. Existing presets
and live audio are unchanged. Parameters are fixed after preparation; do not use
graph replacement as a substitute for smoothed live parameter updates.

## Next gates

1. Evaluate this minimal native execution baseline against FunDSP integration
   before committing to a backend. No dependency decision has been made.
2. Add processor adapters and explicit mono/stereo conversion, algorithmic latency
   metadata/merge compensation, smoothed events and off-thread graph retirement.
   The current Delay is a musical delay, not compensation.
3. Preserve existing rig behaviour with live/render parity tests before activation.
4. Implement the first complete amp/drive voice against a written character
   contract: stage filtering, interacting controls, dynamics, and documented
   omissions. Validate picking response and cleanup, not just spectral similarity.
5. Expose the same rig through Play/Patch views once audio contracts are proven.

Tim Henson is the selected listening reference; exact version/preset remains open.
No branded emulation, hardware fidelity, realtime performance or Pi qualification
is claimed by this routing prototype. NAM oversampling remains deferred.

## Second slice — prepared adapters and merge timing

The graph now accepts control-thread-prepared `StereoNode` implementations bound
to External node IDs. Bindings must be complete, unique and correspond to an
external node. The sample-oriented adapter is an initial correctness interface;
efficient block-based NAM and other heavy DSP adapters are still future work.
External node configuration is not yet an asset/preset registry, so an External
document alone cannot reconstruct its processor after restart.

Compilation propagates declared algorithmic latency and allocates per-edge
compensation at merges. Graph output latency is exposed in samples. Intentional
Delay nodes do not contribute to latency metadata: their echoes remain audible.
Both delay-effect buffers and compensation buffers share the existing memory
budget. External processors own their memory and must be bounded by their future
factory; the graph does not account for those private allocations yet.

This pass supports fixed whole-sample latency only. It does not correct phase,
fractional converter delay, or changing processor latency. All merges currently
align algorithmic latency; immediate-dry/high-latency-effect-return policy is not
yet exposed. These limitations must be resolved before general live integration.

Tests cover parallel plus serial latency, intentional-delay preservation, reset,
missing/duplicate bindings, and compensation budget rejection. No UI/live routing
has changed. Safe graph replacement and smoothed parameter events remain pending.

## Third slice — block execution and live gain

Processing now executes topologically in chunks of up to 128 stereo frames, with
prepared per-node buffers. Native adapters can override `process_block` instead
of being invoked once per sample. The default tick adapter remains suitable for
lightweight algorithms. No extra buffering latency is introduced by chunking.

Gain nodes expose control-thread handles with finite/range validation and atomic
targets. The audio side ramps over 10 ms, independent of callback size. Use
`prepare_at_rate` for the actual device rate; convenience preparation assumes
48 kHz. Only gain is dynamically controlled so far; this is not an automation
event queue or arbitrary effect parameter system.

`ReverbNode` wraps the existing FDN as a stereo wet return with independent channel
state and the existing right-channel spread. Dry/wet routing is explicit through
mixers. Tests compare its left output against the original algorithm exactly,
check silent-channel isolation and clear its tail on reset. Its acoustic onset is
intentional, not reported algorithmic latency.

The block adapter contract is ready for further work, but NAM loading, native-rate
conversion, fault handling and reset semantics are not yet implemented here.
Safe graph switching/off-thread retirement and live integration also remain
pending. External asset serialization and a processor factory are still needed.

## Fourth slice — native NAM and safe graph exchange

`node_runtime::NamNode` loads models on the control thread, processes bounded
blocks through an allocation-free Rust error path, and exposes a fault flag.
Left/right/average input selection is explicit; output is dual mono. This first
adapter only accepts the model's declared native rate; resampled models remain
unsupported until fractional latency is handled. Capture-inherent delay is not
estimated here. Core-internal realtime behaviour still requires instrumentation.

Reset fails closed and requests replacement through the fault flag rather than
pretending to clear/re-prewarm a NAM safely in the callback. Re-prepare the model
off-thread after transport discontinuities. This is a restricted adapter, not
yet a drop-in replacement for the current live rig.

`GraphRuntime` fades down over 5 ms, swaps at a processing boundary, then fades
up over 5 ms. It runs only one graph at a time. One pending and one retired graph
are bounded by queues; further switching waits until the control thread collects
retired resources. Tests verify old processor destructors do not run in processing.
Submit rejects sample-rate/latency changes and returns ownership on rejection.
Stop audio before destroying the runtime/control exchange on a control thread.
No spillover, state transfer or stale-request generation handling is implemented.

Verification: 98 engine-library tests passed, one ignored; Clippy passed. A real
synthetic linear NAM fixture tests native block output and explicit downmix.
Tests also cover fades, queue backpressure, incompatible rate rejection and
off-thread reclamation. UI/editor integration, persistent native node descriptions,
full legacy rig parity and hardware qualification are still outstanding.

## Fifth slice — saved native nodes and offline audition

`RigDocument` persists graph version, sample rate, NAM asset paths/input selection,
and reverb decay. The factory validates topology before loading native processors.
NAM paths must remain relative to the document directory, including symlink
resolution. Asset hashes, relocation UI and quality-tier selection remain pending.

`node_render` loads fresh processor state and exports stereo float WAV without
opening audio devices. Input must be mono/stereo at the rig rate. It refuses to
overwrite existing output. An export I/O error can leave a partial new file.
Output is not limited or normalized; the reported peak warns above 0 dBFS.
Bounds: 1 MiB JSON, 256 MiB input file, three minutes of source, 0–30 s explicit
tail. No automatic conversion or tail estimation; choose enough tail to avoid
abruptly truncating reverb. This is an in-memory offline diagnostic.

From the repository, with a 48 kHz WAV and a new output path:

```sh
cargo run -p pedal-engine --bin node_render -- \
  config/node-rig-example.json /path/to/guitar.wav /path/to/new-audition.wav 5
```

The example mixes dry and reverb branches. A NAM processor entry for an External
node is `{"type":"nam","path":"models/my-capture.nam","input":"average"}`. An optional `"oversampling": 2` (1, 2, 4 or 8; default 1, omitted when 1)
oversamples that node the same way as the live rig's `nam_oversampling`.
The processor key must match the graph node ID. Input choices are left/right/average;
the NAM must declare the document's native rate. Keep models beneath the rig's
directory. Missing or invalid assets fail rather than silently becoming bypasses.

Tests cover serialization, stereo preservation, tail length, invalid paths and
bindings. The CLI integration test reloads a synthetic NAM from a bare document
filename, checks rendered samples and verifies overwrite refusal. UI/editor and
live activation remain pending; the backend is now auditionable offline.

## Sixth slice — cabinet nodes

Saved native descriptors now accept:

```json
{"type":"cabinet","path":"irs/my-cab.wav","duration_seconds":0.2}
```

The node reuses the existing IR loader's filtered resampling, shared stereo level
scaling and truncation taper, and the existing partitioned convolution algorithm.
Duration must be finite and between 20 ms and 1 second; files are bounded at
64 MiB before decoding. Paths use the same containment validation as NAM assets.
Missing/invalid cabinets fail loading rather than silently exposing a raw head.

Mono IRs filter stereo input with separate histories. Stereo IRs apply left to
left and right to right, not a four-channel crossfeed matrix. IR onset is preserved;
the convolver adds no algorithmic buffering latency. This does not infer whether
a NAM already includes a cabinet: choose the cabinet explicitly and avoid stacking
one after a full-rig capture unless that is intentional. Gain and routing remain
separate graph nodes; cabinet low/high-cut controls are not exposed here yet.

Convolution history can now be cleared without allocating/rebuilding FFT plans.
Tests check partition-boundary tail output against the existing convolver,
independent stereo histories with a mono IR, reset, and invalid duration. The CLI
test now reloads both a NAM and stereo IR and checks the complete chain's channel
levels/polarity, as well as refusing to overwrite existing output.

Still pending: node editor, live activation, automatic cab classification, full
processor catalogue, new adjustable amp/drive models, and hardware qualification.

## Offline Patch Lab editor

The header's branching-node button opens Patch Lab. Add gain, musical delay,
mixer, NAM, cabinet, or reverb cards; choose upstream node IDs using input chips.
Edit gain/delay/native settings, remove nodes, and undo/redo edits. This first
editor uses connection chips, not a draggable cable canvas. It does not activate
or modify live audio. Drafts survive closing the panel, but not an app restart.

Import/export uses JSON text and the clipboard, with the same strict document
shape as the native renderer. Save exported JSON beside its assets; enter NAM/IR
paths relative to that folder and press Enter to commit the path. Export is
disabled for incomplete/invalid routing. File existence, symlink containment,
model sample-rate compatibility and prepared latency budgets are still checked
by the native renderer, not by this UI validator. No file picker or UI render
button is included yet. Existing offline CLI remains the audition path.

Focused Flutter tests cover importing the native sample rig, JSON round trips,
atomic rejection of bad imports, feedback/disconnected-node validation, and
add/undo interactions at 480×400. Live activation, freeform graph layout and new
adjustable amp/drive models remain pending.

## Movable node canvas

Patch Lab now opens a pan/zoom canvas with blue chrome node tiles and curved
stereo cables. Drag titles to move nodes. Drag output sockets onto inputs, or
tap an output followed by an input. Connections reject feedback, duplicates,
invalid directions and mixer inputs beyond eight before mutating the draft.
Single-input processors replace their existing connection; mixers add a branch.
Use Controls to inspect settings and remove connections with the existing chips.
The toolbar can switch back to the connection-card view.

Fit Patch arranges nodes in signal-flow order, puts parallel branches on separate
rows, wraps long chains after six columns, and centers the whole layout. The
arrangement is one undoable change and leaves audio connections untouched.
Cable routes bend around intervening nodes, keeping direct dry paths visible
alongside effect branches. Fit includes the cable bends as well as node tiles.
Obstructed cables prioritize the fewest bends before distance, with straight
intermediate routing points removed before rounding the corners.
Zoom buttons help navigate larger rigs. At wide sizes the selected
node's controls sit beside the canvas; on narrow screens they temporarily replace
the canvas until closed. Undo/redo includes node moves. Node positions remain
in memory separately from the strict DSP JSON and are not exported. Audio is
still offline-only; live activation, UI audition/render controls, and new
adjustable amp/drive models remain pending.

## Live activation and clip audition (November 5)

Open Patch Lab → Guitar / Preview. Apply to guitar prepares a snapshot of the
draft on a control connection, then queues a 5 ms fade-down, block-boundary
exchange and 5 ms fade-up. Return to pedalboard restores the existing chain.
Edits require reapplying. Preparation errors leave the existing route intact.
The main power/bypass button still applies (with a short patch bypass ramp),
and the header patch icon is lime while a patch is selected. Status distinguishes
selection from global bypass. The tuner, clean/processed capture, timeline,
metronome and final output protection remain outside the route selector.

The folder button sets an absolute asset root and optional preview WAV path.
NAM/IR node paths stay relative to that root. Blank preview path uses the bundled
guitar clip. Preview renders up to 30 seconds plus a 3-second tail, converting
the source to the patch rate with the existing band-limited resampler. A peak
above 0.89 is attenuated for audition and disclosed in the UI. Playback uses
the existing system-audio player, whereas live guitar uses engine output routing.
Closing the audio panel stops clip playback but does not deactivate a live patch.

Preparation and rendering use a separate socket so the main controls and meters
remain responsive. The bounded exchange keeps retired graph destruction off
the sample-processing path. Live patches currently require the device sample
rate and zero added algorithmic latency; musical delays are allowed. NAMs still
require matching native rates. Processing faults mute the patch until reapply
or return to pedalboard; global bypass remains available. An audio-worker restart
returns to the pedalboard rather than automatically reloading the experimental
patch. Draft JSON, layout and activation are not automatically persisted.

Validation: engine library tests, a real Unix-socket render test with audio
hardware disabled, and Flutter apply/return/error tests. Hardware listening and
CPU/latency qualification are still required. The node catalogue has not changed:
new adjustable amp/drive models and additional effect nodes are still pending.

## Reusable custom nodes

Patch Lab now offers Save as Custom Node and My Nodes. Saving creates an immutable
library revision under XDG_DATA_HOME/aero-dsp/custom-nodes (falling back to
~/.local/share). Referenced NAM/IR files, including nested-node assets, are copied
into that revision's asset directory. Inserting embeds an independent document
snapshot as one named node. Inspect/edit opens that instance's internal graph;
Apply changes affects only the instance. Update from library is explicit and
undoable. Saving a new library revision never silently updates existing rigs.

Nested nodes use the same stereo processing path for offline preview and live
activation. Maximum nesting is four custom levels, with 64 nodes and 192000
delay frames budgeted across the complete hierarchy. Sample rates must match.
Fault flags and processing latency propagate through nested graphs.

Tests cover revision isolation, asset copies surviving removal of source files,
UI insertion/update/undo, nested limits and stereo processing, and two custom
nodes rendered in series through the real engine control socket. Library asset
directories must accompany exported rigs if moved to another machine; portable
bundle export and exposed macro knobs are not implemented yet. Fuzz and other
new creative DSP processors remain separate upcoming work.

## Original Fuzz processor

Add Node → Fuzz provides drive (0–36 dB), bias (-1–1), tone (0–100%), mix
(0–100%) and output (-36–6 dB). Defaults are 18 dB, centered bias, 50% tone,
100% wet and -12 dB output. It needs no NAM/IR file and can be saved inside
Custom Nodes. Settings take effect through the existing Apply to guitar flow.

This is an original biased soft-clipping sound, not a circuit-accurate hardware
clone. First-order antiderivative antialiasing plus a wet averaging stage reduces
aliasing; wet DC cleanup and tone filtering follow. Independent channel state,
finite checks and reset behavior are tested. One nominal sample of latency is
reported, with a precisely one-sample-delayed dry endpoint. Frequency-dependent
filter/nonlinear phase remains: compensation does not promise broadband nulling.

Live patches now accept up to 64 samples of added algorithmic latency. Bypass
dry uses a matching bounded delay, status exposes latency_samples and the audio
panel displays it. The patch-specific latency is not added automatically to the
Studio recording compensation setting yet. Six DSP tests include one targeted
high-frequency alias comparison; this is not comprehensive perceptual or alias
qualification. Engine library and socket preview tests plus Flutter control,
validation and custom-node tests pass. Physical guitar listening remains needed.

## Building and comparing complete rigs

Add Node groups processors by role. Overdrive passes the low fundamental through
a clean path and clips the upper band. Edge Amp has a separate drive, tone,
touch compression and master. Its envelope responds differently to the attack
and sustain of a note. Add a cabinet node for speaker coloration. Both processors
report one sample of nominal latency. Chorus, flanger and octave-down have
intentional wet-path delays; tremolo modulates amplitude. None requires a NAM.

Use Load starter rig to try Glass Arpeggios, Split Drive, or Octave Motion.
Loading changes the draft and can be undone. Apply to guitar activates it.
When saving a Custom Node, expose up to four numeric parameters and give each
a name. Its inspector can then change those values for this copy of the node.

Compare rigs stores two draft snapshots during the editor session. Capture A,
close the workbench, change the draft, reopen it and capture B. Choose the bundled
DI, a WAV, or the latest saved clean take. Play A and Play B render the same
performance. The engine matches stereo RMS after processing over frames where
source peak exceeds -60 dBFS, and applies shared attenuation for peak headroom.
This preserves input gain and clipping character. Recall to draft returns the
chosen rig and its asset directory for further editing, saving, or live use.

## Transpose and creative effects

Patch Lab now includes Transpose, Envelope Filter, Ring Modulator, Rhythmic
Slicer and Bitcrusher, including parameter validation, inspector controls,
integer-safe custom-node macros, saving/importing and live/preview processing.
See [the usage and limitations guide](patch-lab-creative-effects.md).

Transpose is an original RustFFT phase vocoder. Short-window Signalsmith
pitch shifting was evaluated first but did not pass simultaneous-note pitch
checks. The native implementation keeps independent input-bin phase accumulators
when output bins collide. Seven active tests check low chord tuning, stereo
phase/gain, arbitrary block partitioning, reset, Nyquist rejection and window
normalization/latency. It remains a first implementation: transient smearing and
partial-level changes need listening evaluation. At 48 kHz Fast adds 2048 samples
(42.7 ms), Smooth 4096 (85.3 ms); zero semitones is an exact undelayed bypass.

The live route now reserves half a second of dry history off-thread and accepts
graphs up to that algorithmic latency ceiling. Parallel branches and global
bypass use the prepared graph's delay. Studio wet-take compensation remains
separate. Slicer runs on its own BPM clock; transport/MIDI synchronization is
not implemented.

Integration testing also fixed nested custom-node JSON parsing: processor IDs
are explicitly decoded from string keys, including inside tagged descriptors.
Actual control-socket previews now cover each new effect inside a custom node.

Validation on 2026-11-07: 137 engine library tests passed (one pre-existing
manual test ignored), saved/nested effect graph tests, renderer and preview/
comparison socket tests passed. Fourteen focused Flutter tests passed; analysis
has one pre-existing deprecation notice in tone_workbench.dart. The bundled
guitar clip rendered through all five effects (both transpose modes) with finite,
non-silent stereo output. These checks did not involve listening or Pi hardware.

A local release microbenchmark at 48 kHz/64 frames measured all five nodes in
series at 28.3 microseconds mean and 104.5 microseconds p99 per 1333.3-microsecond
audio block. This measures processing cost, not monitoring delay or a guaranteed
real-time deadline. The reproducible harness is
`cargo test -p pedal-engine --release --test creative_patch profile_effects_in_64_frame_blocks -- --ignored --nocapture`.

## Live controls and Rig → State terminology

Rig → State is the agreed naming for future grouped switchable sounds. This
slice implements live numeric controls; multiple States, shortcuts/footswitches,
and dropping disconnected nodes onto cables are separate upcoming work.

After Apply, the editor owns the active rig's unique revision. Supported slider
changes are sent at a bounded rate with one update in flight and the latest
pending values retained. Undo/redo and nested custom-node macros use the same
path. Activation responses survive closing the audio panel, and late responses
cannot reclaim a newer rig's controls. The toolbar distinguishes live controls,
unapplied structure, and failed updates.

`update_patch` accepts the revision, complete draft document, and asset folder.
The engine validates all live values and compares the remaining structure before
publishing numeric targets. It rejects stale revisions, changed assets/wiring,
and preparation-only controls without replacing or resetting the active graph.
Live handles use atomic targets; DSP smoothing is 20 ms (existing Gain nodes use
10 ms). FFT/model/file preparation and control locks stay off the audio thread.
Ordinary changes preserve reverb tails, filter history, and oscillator phase.

Coverage includes smooth audible gain updates, retained reverb tails, nested
updates, all-or-nothing validation, stale revisions, continuous UI dragging,
changes during activation, custom macros with undo, and closing the panel while
Apply is pending. Hardware listening is still needed to judge the feel of the
controls with a real performance.

Validation: 143 engine library tests passed (one existing manual test ignored),
plus saved/nested graph, renderer and control-socket integration tests. Nineteen
focused Flutter tests passed. Static analysis has no new warnings; the existing
tone_workbench.dart deprecation notice remains.
