# NAM level defaults and normalization

Drive NAM and NAM Head use TONE3000's neutral starting levels: input and output
trim at 0 dB, fully wet signal, and flat post-model six-band EQ. The Head's
additional bass/mid/treble controls also start at 0 dB. Drive input trim, Drive
output level, and Head volume now support −24 through +24 dB. Drive changes the
level sent into a fixed capture; it cannot move the captured hardware's knob.

## Input calibration and hand-off

With calibration on, each NAM is staged on its own, as in TONE3000. The first
model in the chain receives `reference − its input_level_dbu`. A Drive that
feeds a Head hands off at its true output level (`output_level_dbu −
reference`), and the Head then applies `reference − its input_level_dbu`; with
both levels known the reference cancels to `Drive output − Head input`. When
only one side carries metadata, that side is still applied: a Drive without
`output_level_dbu` normalizes as usual and the Head keeps its own input
calibration. A hand-off suppresses that Drive's normalization; a lone Drive
or Head (the last NAM) keeps normalization. A Drive mix below 100% disables
calibration, since a parallel dry/wet blend has no single output level.

## Head tone stack

BASS, MID and TREBLE follow the NAM plugin's tone stack (as TONE3000 does):
150 Hz low shelf, 425 Hz bell (Q 0.7 when boosting, 1.5 when cutting) and
1.8 kHz high shelf, so dB settings carry over from NAM and TONE3000.

## Capture normalization

New rigs enable **NORMALIZE** independently for Drive and Head. The control bay
for each slot offers an on/off switch. This is metadata-based gain adjustment,
not a compressor or a continuously measured loudness target:

```text
correction dB = clamp(-18 - capture loudness, -24, +24)
```

The −18 dB reference follows TONE3000's default target. Only finite loudness
metadata between −100 and 0 dB is accepted; missing or invalid metadata yields
unity gain. The adjustment follows the NAM and precedes post EQ, dry/wet mix,
and output trim. It never scales the Drive's dry branch. Changes use 10 ms gain
smoothing, with the initial gain seeded on the first processing block after
calibration routing is known. Clearing a slot returns its normalization gain
to unity immediately.

Existing presets and engine-state files without the new flags deserialize with
normalization OFF, preserving their sound. Explicit ON/OFF settings save and
restore with the preset and survive offline restart. New presets start ON.
To use normalization in an existing rig, enable it in each desired NAM slot
and save. Normalizing Drive can change how hard it drives Head; its level knob
remains available for deliberate boosts.

## Calibration interaction

When interface calibration is explicitly enabled and the fully wet active Drive
and Head have valid output/input level metadata, the Drive retains the calibrated
interstage level instead of adding normalization. Head normalization still
controls the terminal NAM level. Invalid calibration metadata (nonfinite or
outside −60 to +60 dBu) cannot trigger that override.

AERO retains its existing conservative calibration rules: users supply a measured
interface reference, partial dry/Drive mixes disable automatic interstage
calibration, and calibration gain is bounded. These differ from TONE3000's more
general chain calibration, so this is not a claim of identical routing for every
configuration. The global input trim remains independent.

## Implementation and reference

The shared fixed-rig path applies the behavior in live playing and offline
previews/reamping. Direct NAM nodes in Patch Lab keep their existing behavior.
No neural model weights, oversampling preferences, hardware settings, or capture
files are changed. The audio path adds no locks, allocations, or buffering.

Behavioral reference: TONE3000 commit
`1bbb21b5630bf359c348c4f16ffda7a47695db2d`,
[block defaults](https://github.com/tone-3000/tone3000-plugin/blob/1bbb21b5630bf359c348c4f16ffda7a47695db2d/plugin/include/ChainBlock.h)
and [normalization/calibration processing](https://github.com/tone-3000/tone3000-plugin/blob/1bbb21b5630bf359c348c4f16ffda7a47695db2d/plugin/src/Processor.cpp).
The implementation is original Rust. See also [NAM EQ](nam-eq.md).

## Verification

203 engine tests pass; two pre-existing tests remain ignored. Tests cover valid
and invalid metadata, correction bounds, wet-only mixing, bypass, model swaps,
clearing a slot, calibrated handoff, old/new preset persistence, control ranges,
and real NAM files rendered through both live and offline paths. The 42 selected
Flutter tests pass, including actual 480×320 control bays with loaded capture
carousels, normalization switches, negative gain, and the existing EQ/alignment
controls. Physical guitar listening remains to be checked after relaunching.
