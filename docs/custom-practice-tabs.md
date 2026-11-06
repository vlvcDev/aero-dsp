# Custom practice tabs

Open the lesson menu, then **My practice tabs** (library icon). Create a tab,
name it, select a tuning and tap a string/time cell to enter a fret (0–24).
New tabs start with four empty bars in 4/4. Add or remove bars up to 64.
Removing a populated bar requires confirmation. A note can be edited or deleted
by tapping its position. Only one note is allowed at each time position;
placing a note on another string at that position replaces the previous note.

Whole, half, quarter, eighth and sixteenth are placement grids, not durations.
Changing grid does not quantize or remove existing notes; off-grid notes remain
visible and editable. Empty positions are rests. Changing tuning retains frets
and changes the expected pitches accordingly. Tabs are stored in musical ticks,
independent of tempo.

Choose **Practice** in the library to set BPM (30–300) and repeats (1–32).
One four-beat count-in precedes the run; repeats follow continuously without
another countdown. The tab moves past a fixed lime marker, including for daily
drills. A Stop button cancels the run. Custom scores show hits and misses, and
never write to daily practice history.

Scoring reuses clean-input pitch telemetry (confidence ≥0.62, ±35 cents), with
a late window of at most 200 ms, shortened for closely spaced notes. This is
pitch-window scoring, not validated picking-onset detection: a sustained pitch
can satisfy repeated identical notes. String/fret choice and duration are not
graded. Hardware/telemetry latency can affect timing; this is not a sample-exact
performance assessment.

The offline library is `practice-tabs.json` beside the existing practice-history
file (normally `$XDG_STATE_HOME/lost-audio/`, or `~/.local/state/lost-audio/`).
The legacy directory is retained for consistency with existing user data.
Saves use a temporary file and atomic rename. Load errors are shown rather than
silently overwriting a damaged library. Failed saves keep the editor open with
its edits intact. Closing unsaved edits asks before discarding them.
