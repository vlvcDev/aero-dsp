# Local guitar-to-tab drafts

Studio guitar tracks now have a **Generate guitar tabs** action (the music-list
icon). Select the source clip, confirm the recorded tuning, then Generate. The
job uses the clean DI, including the clip's source-in/source-out, not the wet
rig, backing or metronome. The project tempo is a reference, not an instruction
to change the original note timing.

## Installation

From the repository root, run `bash tools/setup-transcription.sh`. It needs `uv`
and internet access once. It installs Python 3.11 and Spotify Basic Pitch 0.4.0
with ONNX CPU inference in `.transcription-venv`; no API key, audio upload or
runtime network request is needed. Restart the catalog and UI after updating.

The system Python remains unchanged. Basic Pitch's package metadata requests
TensorFlow on Linux/Python 3.11 even when using ONNX, so the setup explicitly
installs inference dependencies and the unchanged Basic Pitch wheel without
that unused backend. `setuptools<81` supplies the older resampler's
`pkg_resources` dependency. An external deployment can set
`PEDAL_TRANSCRIPTION_PYTHON` to its dedicated worker interpreter's absolute path.

## Storage and processing

- The catalog validates the riff ID and confines source paths to the Riff Vault.
- One subprocess job at a time, lower scheduling priority, one ONNX thread,
  and numerical library thread limits. This reduces contention but is not a
  guarantee against audio xruns on a loaded Pi; hardware qualification is pending.
- Maximum clip length: 180 seconds. Worker timeout: 15 minutes.
- Closing the tab window leaves the job running. Open it and generate the same
  clip/tuning/BPM again to reattach or reopen the cached result.
- Results are saved under `<riff directory>/transcriptions`: JSON note events,
  plain-text six-line tabs, and unquantized MIDI. Cache identity includes source
  path, file size/modification time, clip range, tuning, BPM and pipeline version.
- Interrupted jobs are retryable after restart; completed results survive
  offline restarts. Source recordings are never modified.

## Interpretation and limits

The **Tab Workbench** displays six strings on a time-scaled, horizontally
scrollable score. Duration tails show how long detected notes last. Beat lines
use the project BPM and the clip's timeline position. Zoom and mouse/touch
dragging are supported; note positions are not cosmetic/equally spaced columns.

- Tap a fret, then Edit to change pitch, string/fret and start/end in milliseconds.
- Add/delete notes, undo/redo (50 undo steps), and restore the model's original notes.
- Optional **Snap starts to 1/16** aligns note starts to the project beat grid,
  preserves durations where possible within the clip, and can be undone.
- Unassigned pitches appear in a review list and can be corrected there.
- **Play/Stop** uses the native timeline mixer. The cursor follows reported audio
  frames, auto-scrolls, and highlights sounding notes. Select clean DI or the
  recorded rig, choose a precise section in milliseconds, and optionally loop it.
  This plays the original recording, not a synthesis of edited notes. Stop the
  Studio recording before opening the editor. The editor owns its preview
  playback and stops it when closed; the Studio transport is paused on entry.
- **Save edits** persists a new correction revision and regenerates text/MIDI
  exports. Original model notes and exports are retained. Closing or choosing a
  different clip prompts before discarding unsaved edits. Concurrent stale saves
  are rejected rather than overwriting newer corrections.

The downloadable text format still uses attack columns, not equal beat spacing.
The on-screen score is proportional to time. Edited MIDI contains note pitches,
timing and estimated velocity; raw pitch-motion arrays remain in JSON but are
not synthesized into corrected MIDI pitch bends. Corrections are not automatically
certified as training labels, and nothing is uploaded or used to train a model.

A bounded fingering search assigns detected pitches to the selected six-string
tuning (standard, Drop D, half-step down, whole-step down or Drop C), frets 0–24.
It discourages hand movement and stretches and avoids occupying the same string
with overlapping notes. Unassignable notes remain in JSON and the review list.
Audio does not uniquely reveal the original string/fret. A `~` marks pitch motion,
not a verified bend, slide, hammer-on or vibrato. Capo, custom tunings, formal
engraved rhythm notation, audio time-stretch controls and synthesized tab
playback remain outside this editor. No calibrated accuracy/confidence
percentages are claimed. Manual edits can create overlapping/impractical
fingerings; the editor does not silently rewrite user corrections.

## Tests

`python3 -m unittest discover -s services/catalog/tests -v`

For actual ONNX inference and export/cache verification:

`AERO_TEST_BASIC_PITCH=1 .transcription-venv/bin/python -m unittest discover -s services/catalog/tests -p test_transcription.py -v`

The real-model smoke test uses a generated A3 tone, validates timing bounds,
MIDI/text exports and reopening without the runtime. It is not a guitar accuracy
benchmark. Backend tests also cover saved corrections, invalid notes, revision
conflicts, restoration and exports. Flutter tests cover 480×320 scrolling,
note/fret/time editing, undo/save, native playback offsets and close protection.

Upstream: https://github.com/spotify/basic-pitch (Apache-2.0; retain the package's
license and notices when distributing it).
