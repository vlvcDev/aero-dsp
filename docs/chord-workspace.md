# Chords workspace

The **CHORDS** tab contains an offline chord atlas and editable progression lab.
The local catalog service must be running; internet and API credentials are not
required for the atlas, offline ideas, saved progressions or synthesized preview.

## Atlas

- Standard six-string tuning only (E A D G B e, low to high).
- All 12 roots, each with major, minor, dominant seventh, minor seventh, major
  seventh, sus2, sus4, power, add9, 6, m6, 9, m9, maj9, diminished and augmented:
  192 chord definitions.
- Common open shapes plus movable E/A-family voicings; alternate shapes retain
  their actual fret positions. Finger numbers, barre indicators, open strings,
  muted strings, string labels and fret numbers are shown.
- The library is authored from chord intervals and conventional shape templates,
  not scraped copyrighted chord charts. Tests verify every shape's pitch classes.
- Compact three/four-string voicings include different bass notes (inversions),
  labeled by bass note. They contain all named pitch classes and use at most four
  separate fretting fingers within a four-fret window. Ninth chords use fuller
  movable shapes; no omitted-tone shell voicings are implied. Pitch validation
  does not replace personal comfort checks. Old shape indices remain unchanged.
- Enharmonic roots use one spelling (e.g. Eb instead of D#), not full contextual
  notation. No alternate tunings, capo or left-handed diagrams yet.

## Selected-chord variations

Tap **Select chords**, mark any progression steps, then choose Darker, Brighter,
More colorful, Simpler, More tension, More resolved, or Smoother voicings.
The blue-chrome comparison bay generates a suggestion automatically and offers
Hear original / Hear variation, Stop, Try again, Apply variation and Cancel.
Closing the bay also stops its preview. Both versions play the full progression
at the same BPM so the changed passage can be heard in context.

Variations are deterministic, offline harmonic heuristics, not AI requests.
Tension suggests a dominant of the following original chord; resolution targets
the chosen tonic, inferring minor from an existing minor tonic when present.
Darker/brighter primarily change chord quality; they do not guarantee a mood.
Try again cycles finite choices; some actions can have only one useful result.
Unselected chord IDs, shapes and all durations are protected. Smoother voicings
changes shapes only, considering unchanged neighbors and the loop boundary.
Apply is explicit; Cancel leaves the draft untouched. Undo variation restores
the most recently applied variation, and disables after intervening edits to
avoid overwriting later work. Saved progressions retain the resulting shapes.

## Progressions

Add up to 16 chords, select alternate shapes, move chords left/right, remove
chords and choose 1–8 beats per chord. Tonal-center selection transposes the
progression; it resets shapes to the lowest library voicing for the new roots.
Saved entries retain name, BPM, tonal center, chord order, shapes and durations.
They also retain section label, mode, and each chord's performance pattern/grid.
Save is explicit; drafts survive workspace switching while the app stays open.
Replacing an unsaved draft prompts first. Drafts are not recovered after closing
the app. Local storage uses atomic replacement beside the existing practice data:
`~/.local/state/lost-audio/chord-progressions.json` (or the configured state path).

Offline ideas rank eight common harmonic templates by keyword overlap with the
mood/genre/theme prompt. Labels describe possible feelings, not objective musical
rules. Suggestions are fully editable.

**Ask AI** is optional and uses the existing `PEDAL_OPENAI_API_KEY` and
`PEDAL_OPENAI_MODEL`. The prompt, tonal center, shape preference, relevant local
recipes and up to two transposed example progressions are sent; no recorded audio
or saved user library is uploaded. A strict chord-ID/timing schema, followed by local
validation, prevents invented chords or fingerings from entering the progression.
This follows OpenAI's [Structured Outputs guidance](https://developers.openai.com/api/docs/guides/structured-outputs).
AI availability and live response quality still depend on the configured account.

## Context-assisted suggestions

- `pedal_catalog/chord_guidance.py` holds nine original, editable style recipes:
  pop, indie, jazz/soul, ambient, metal/rock, folk/country, blues, cinematic and funk.
  Each contains tags, relative harmonic patterns, timing guidance and provenance.
  These are project-authored starting points under the repository license, not
  scraped material or artist-specific evidence. Genre/mood labels are not rules.
- Case-insensitive token matching retrieves at most three relevant recipes.
  Unknown requests receive no unrelated recipe/example fallback; the model uses
  its general knowledge. This lightweight retrieval needs no vector database,
  embedding request or extra package. It is not semantic search and can miss
  synonyms or retrieve a genre mentioned negatively; the model is instructed to
  prioritize the user's request over optional recipes.
- AI chooses 2–16 chord steps with integer quarter-note durations of 1–8 beats.
  Local validation rejects missing/invalid durations and unsupported chords.
  Timing is harmonic duration, not a strumming pattern or a generated drum groove.
- **Suggested shapes** offers comfortable transitions, prefer open, or prefer
  movable. Both offline and AI ideas use a small deterministic optimizer over
  known shapes, minimizing fret travel, stretch and barre penalties over the
  full loop. Preferences are soft: an open voicing may not exist for a chord.
  This is an ergonomic heuristic, not personalized fingering or full voice-leading
  analysis. All shapes remain manually editable. The preference lasts for the
  workspace session; saved progressions persist the actual chosen shapes/beats.
- The configured model is unchanged. No automatic learning from saved progressions,
  new paid service, live web retrieval or model training has been introduced.

### Listening comparison before changing models

Use the same tonic, shape preference and prompts for baseline/enriched/smaller-model
trials: bright beginner pop; wistful indie with a borrowed chord; warm resolving
jazz; spacious ambient; aggressive thrash; acoustic folk with open shapes; twelve-bar
blues; unresolved cinematic suspense; sparse funk; an unfamiliar genre; and a mixed
mood request. Blind-rate musical fit, variety, transitions and timing, alongside
latency/cost. Automated tests cover payload context, limits, transposition, valid
shapes, preference selection and duration preservation; they cannot establish
subjective musical improvement. Live model listening comparisons remain manual.

## Preview

### Phrase / section sketches

The **PHRASE / SECTION SKETCH** panel separates harmony from performance.
Choose ascending, descending, up/down or string-skipping arpeggios; steady chord
hits, offbeats, muted bass hits; or held chords. Quarter/eighth/sixteenth placement
sets the attack grid. A pattern starts fresh at each chord and ends at its boundary.
Offbeats skip every other placement; muted bass uses the lowest two sounding
strings and a short synthesized decay (not a realistic palm-mute guitar model).

Apply to the currently edited chord, marked chords in selection mode, or all
chords when browsing freely. This preserves the harmony and beats. **Show phrase
tab** and **Play phrase** derive the same note events; duration bars and gaps
represent actual sustain/rest time. The tab is horizontally scrollable. Edit the
pattern, grid, shape or chord duration and regenerate; individual generated-note
editing is not included in this first pass. **Play progression** remains the
plain held-chord comparison, and **Hear chord** remains a single chord preview.
Variation A/B previews honor performance patterns when present.

**Voice-led transitions** in Select chords chooses known shapes by an
order-preserving pitch-distance match, with a cost for adding/removing voices,
plus ergonomic penalties. It keeps chord identities, durations, patterns and
unselected shapes intact. It optimizes the loop boundary too. This is distinct
from **Smoother voicings**, which primarily minimizes fret-hand travel; it does
not generate melodic passing notes between chords.

The explicit key/mode context supports major, natural minor, Dorian and
Mixolydian. Older saves and suggestions without a mode use **Mode unspecified**
rather than guessing. Notes outside a selected mode are flagged, not removed;
borrowed harmony is allowed. This context is stored locally with Intro / Verse /
Chorus / Bridge / Outro / Custom section labels. This version does not yet add a
multi-section song arranger or enforce diatonic constraints on AI/variation
requests. Those can later operate against the saved context explicitly.

`chords_phrase` returns a version-1 phrase: PPQ 480, total duration in ticks,
chord spans and note events containing tick, duration, string (0=low E), fret,
MIDI pitch, velocity, articulation and originating chord step. Notes at the
same tick are simultaneous; missing events are rests. BPM stays separate, so
tempo changes don't rewrite events. `chords_phrase_preview` renders these events
off the audio thread and uses the existing native timeline transport. No cloud
requests or new dependencies are needed. The 4/4 reference, version and
beat-relative positions provide an interchange foundation for future section
arrangement, practice and Studio—not a second saved audio-only representation.

Existing saved shapes remain stable. Pattern operations and voice leading do
not silently rewrite the user's chosen chords to fit a scale.

The catalog synthesizes the actual notes of the selected shapes as a quiet,
plucked harmonic reference with short strum offsets. It is not a guitar model.
One mono PCM scratch WAV is atomically replaced, with no growing variant cache.
Synthesis happens off the audio thread. The native timeline engine provides
sample-clock playback, looping and the configured output route; no fallback
system-audio player is started. The engine must be running for audible previews.

BPM is 40–240. Edits are disabled during playback. Preview refuses to take over
active Studio playback/recording, and stops when leaving the Chords workspace.
The displayed current chord is driven by native playback position, not a UI timer.
