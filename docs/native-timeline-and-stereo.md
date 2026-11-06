# Native timeline and stereo audio

## Using it

- Studio Play now uses the same output as the live guitar. It needs the audio
  engine running; it no longer falls back to separate `pw-play` processes per clip.
- Use the red Record button in the studio header, select a guitar track if needed,
  play through the count-in, then press Record or Stop to save. The take is appended
  as a new non-destructive clip, not written over an existing clip. Recording stores
  mono DI and stereo rig audio; the track's DI/rig playback choice is respected.
  Saved takes also have Riff Vault metadata for reamping/backing-track workflows.
- The existing “Save latest guitar take” remains available separately.
- Hold the header Record button for recording alignment. Enter a **measured**
  round-trip delay in milliseconds (0–2000). This is saved with the project and
  affects new takes only. Leave zero if unknown; changing device/routing/buffer
  settings invalidates a previous measurement. Compensation moves the clip earlier;
  at timeline zero it trims its leading view without deleting source samples.
- The metronome toggle works while playing. Stop before changing BPM/count-in.
  The counter is generated in the engine; it does not become part of a take.
- Mono tracks use equal-power pan (−3 dB per side at center). Stereo tracks use
  balance: unity left/right at center, attenuating the opposite side when moved.
  Volume, pan, mute and solo changes are smoothed over about 10 ms. Clip edits are
  loaded on the next playback start; timeline ruler seeking starts native playback
  at the requested position. Seeking while recording is rejected.

## Stereo path

The guitar input, gate envelope, compressor, octaver and NAM drive/head stay mono.
The cabinet produces left/right audio from a mono or two-channel WAV IR. Both
channels retain independent EQ, chorus/flanger delay lines, delay and reverb state;
the stereo looper shares one transport position. Right-channel modulation phases
and reverb delay lengths differ to produce width from a mono source. A mono IR
remains centered unless a later effect adds width.

Stereo IR import uses one gain scale for both channels, preserving their relative
level and phase. It does not average the IR channels. Four-channel LL/LR/RL/RR
convolution matrices are not supported. Recording, reamping and trimming preserve
the wet stereo image, and a mono hardware output receives an explicit downmix.
Peak protection uses linked gain, not independent left/right limiting. It remains
a sample-peak limiter, not a true-peak limiter.

## Scheduling and resources

`timeline.rs` owns prepared assets, source offsets, clip ranges, track controls and
the sample clock. WAV loading/resampling, arrangement preparation and file saving
happen off the audio callback. Prepared sessions cross a non-blocking mailbox;
retired sessions are reclaimed on the control thread. Rendering uses bounded
scratch storage and processes only overlapping portions of each clip.

Initial bounds: 64 tracks, 4096 clips, 10 minutes of unique decoded assets, and
256 MiB per input WAV. Assets are shared between clips. Recording preallocates
three minutes of DI + stereo wet samples (about 104 MB at 48 kHz), then stops
automatically. This is an in-memory short-session mixer, not disk streaming.
Unsaved recordings do **not** survive an application/process crash.

Audio underruns stop the timeline rather than silently continue with uncertain
alignment. Device-worker shutdown retains captured samples in memory; Stop can
export them. File-write failures retain the pending take for retry. The device
clock establishes sample alignment, but USB/backend/converter latency still needs
measurement; interface names or gain-knob positions cannot supply it.

## Verification and remaining checks

Automated tests cover exact offsets, callback boundaries, loops, pause, pan/solo,
smoothed live controls, count-in exclusion, DI/wet taps, compensation at timeline
zero, control/audio handoff, interrupted-take export, stereo output routing and
linked limiting. A stereo IR with opposite-polarity channels survives live capture,
offline rendering and trimming. Chorus/flanger/reverb tests verify nonzero stereo
width and finite output. A widget test records, stops and persists the resulting
clip placement. Flutter analysis and Linux release builds are checked separately.

Real Scarlett/Pi listening, physical loopback calibration, disconnect tests and
full stereo+NAM+timeline load measurements remain necessary. Earlier mono effect
benchmark numbers do not qualify this expanded stereo path. Stereo effects and
two-channel convolution cost more CPU; NAM inference itself is not duplicated.
