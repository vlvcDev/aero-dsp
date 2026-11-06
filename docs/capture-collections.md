# NAM capture collections

The TONE3000 browser selects all compatible captures by default. Users can clear
the selection and check a subset. Before downloading, a confirmation displays
the number of captures; aggregate size is explicitly unknown because the current
catalog response does not provide it. Downloads run sequentially with progress.
Already indexed captures with the same tone/model IDs are reused. Completed files
remain cached if a later download fails; retry continues using those files.
Closing the browser stops the batch after its current request finishes.

The local tone library groups captures by **TONE3000 tone ID**, not by uploader
name, filename similarity or download batch. Groups expand to show their saved
captures. Later downloads join the same group. Local imported files without
TONE3000 metadata remain standalone.

Drive/Head controls show a blue-chrome capture selector for the loaded group.
Arrows cycle captures, swipes move between them, and tapping the name opens the
cached list. Only cached captures appear; switching does not require a network
connection. The selector changes its active label only after the engine accepts
the load. Switching uses the existing engine model-loading path and preset path
storage. Save the pedalboard preset to retain the selected capture in that preset.

Each downloaded `.nam` has a `.nam.json` sidecar containing tone ID, model ID,
capture label and, when known, listing name. Copy the sidecars along with the
NAMs when moving the library. Scanning hydrates group metadata from those files,
including after rebuilding SQLite. Refreshes no longer overwrite known source
labels with `local`. No model file is moved or renamed by grouping.

When downloading another capture, legacy siblings whose filenames exactly match
the previous TONE3000 download naming scheme are linked using the current remote
capture listing. Renamed legacy files with no metadata cannot be reliably linked
automatically. No inference based on just similar names is performed.

The browser fetches subsequent API pages, rather than silently limiting a
collection to the first 20 files. Safety guards reject repeated pages or more
than 100 pages. Existing NAM architecture compatibility filtering is unchanged.

Captures may differ in channels, microphones or cab inclusion as well as knob
settings. The selector preserves uploader labels and does not interpolate
settings or automatically change IR routing. Check the cabinet configuration
when switching between captures of different signal chains.

Verification includes subset downloads/cached reuse, offline carousel switching
and failure retention, a 480×320 control view, persistent grouping after rescan
and index rebuild, legacy sibling adoption, and multi-page listing.
