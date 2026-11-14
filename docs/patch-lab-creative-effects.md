# Creative effects in Patch Lab

These nodes run in live patches and clip previews, and can be saved inside a
Custom Node. Add them from **Patch Lab → Add Node**, connect them into the signal
path, then use **Apply to guitar**. Once the rig is active, supported numeric
controls update the sound as you drag them. The toolbar says **Tuning live** when
this editor controls the active rig. Undo/redo and exposed custom-node controls
also update live. New wiring and settings that require preparation show
**Reapply rig to hear structural changes** and remain draft changes until Apply.

Live controls include gain, drive/tone/output, wet mixes, modulation rate/depth,
flanger feedback, reverb decay, envelope-filter controls, ring-modulator frequency,
slicer BPM/depth, and bitcrusher sample rate. Transpose, octave mode, bit depth,
slicer pattern/division, delay length, NAM/IR loading and cabinet length still
require Apply. Live changes preserve the running effects' history and use short
parameter ramps. A failed/disconnected update is shown instead of silently
reapplying the rig.

## Transpose

Place Transpose before distortion, amp or NAM processing to change the tuning of
the entire incoming guitar signal. It processes chords as well as single notes.

| Shift | Result when the guitar is physically in E standard |
| --- | --- |
| -1 semitone | E♭ standard |
| -2 semitones | D standard |
| -3 semitones | C♯ standard |
| -5 semitones | B standard |
| 0 | Unchanged; exact pass-through with no added node delay |

The range is -12 to +12 semitones. This changes every string equally; it does not
create alternate tunings that require different shifts on different strings.

**Fast** uses a shorter analysis window; **Smooth** uses a longer window for
denser material. At 48 kHz their nominal added delays are approximately 42.7 ms
and 85.3 ms, respectively, before audio-interface buffering. These are audible
monitoring delays, not claims of low-latency pedal equivalence. Large shifts and
sharp transients can change timbre or introduce artifacts. Use the live patch
status for the actual prepared graph delay.

The engine uses an original stereo phase vocoder built on RustFFT. Each input
frequency bin retains its own phase accumulator when shifted bins overlap,
avoiding the frequency averaging that can detune simultaneous notes. Frequencies
shifted above Nyquist are discarded. The analysis-window length is reported as
algorithmic latency; parallel branches and global bypass are delayed to match.
Zero shift skips the processor. FFT plans, scratch buffers and bypass history
are prepared before audio processing. Live graphs have a bounded half-second
algorithmic latency limit; this is a safety ceiling, not a target.

This first version can soften pick attacks and change the relative levels of a
chord's partials. It does not preserve formants. Pitch accuracy on synthetic
chords is tested, but it is not yet listening-qualified against dedicated
commercial detuning pedals.

Studio's existing recording-delay setting is unchanged. Patch-specific latency
is not automatically compensated in recorded wet takes; do not assume their
alignment with the clean DI is corrected by graph branch compensation.

## Envelope filter

An upward, playing-dynamics-controlled filter sweep. Use Sensitivity to adapt to
your pickups, Base frequency to set its resting position, Resonance to emphasize
the vowel character, and Decay to set how quickly the filter closes after a note.
Mix blends with the original signal. Start before heavy distortion so the input
still has picking dynamics. This is an original auto-wah design, not a hardware
circuit recreation.

## Ring modulator

Multiplies the input by a sine carrier. Frequency controls the metallic/bell-like
sidebands; Mix blends the effect with the dry guitar. The carrier is shared across
stereo channels to preserve their relationship. It is deliberately inharmonic at
many settings and does not track the note being played.

## Rhythmic slicer

Chops sustained notes into **Pulse**, **Offbeat**, or **Gallop** patterns. Set BPM,
quarter/eighth/sixteenth-note subdivision, and Depth. Short ramps soften the gate
edges. Its clock starts at patch activation/reset and runs independently of
Studio, the metronome, MIDI, and playback transport. Reapplying a patch restarts
its pattern; BPM alone does not phase-lock it to a backing track.

## Bitcrusher

Reduces bit depth (4–16 bits) and holds samples at a reduced effective rate
(1–48 kHz, limited by the device rate). Mix blends with the dry guitar. The
quantization noise and aliasing are intentional parts of the effect, rather than
a transparent sample-rate conversion. Lower bit depths are more aggressive.

## References

- [Driedger and Müller: A Review of Time-Scale Modification of Music Signals](https://www.audiolabs-erlangen.com/content/05_fau/professor/00_mueller/06_projects/90_siamus/2016_DriedgerMueller_TSMOverview_AppliedSciences_ePrint.pdf)
  explains phase-vocoder frequency estimation and its quality tradeoffs.
- [Signalsmith Stretch](https://signalsmith-audio.co.uk/code/stretch/) was
  evaluated for Transpose. Short analysis windows failed our simultaneous-note
  pitch checks, so this node uses the native phase vocoder. Existing octave and
  time-stretch features continue to use Signalsmith.
- [EHX Q-Tron manual](https://www.ehx.com/wp-content/uploads/2021/07/q-tron-manual.pdf)
  describes the playing-dynamics/filter relationship used as a product reference.
- [BOSS SL-2](https://www.boss.info/us/products/sl-2/) illustrates rhythmic slicing
  as an effect family. Our initial slicer has three original patterns and does
  not implement that pedal's pattern library or MIDI synchronization.

Validation covers pitch content, stereo behavior, reset and block boundaries,
parameter bounds, saved/nested graphs and editor controls. Automated checks are
not a substitute for guitar listening tests or a Raspberry Pi performance test.
