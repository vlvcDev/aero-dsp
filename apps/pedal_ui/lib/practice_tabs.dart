part of 'main.dart';

/// Positions use sixteenth-note ticks; tempo belongs to a practice run only.
class PracticeTabNote {
  const PracticeTabNote(this.tick, this.string, this.fret);
  final int tick;
  final int string;
  final int fret;
  Map<String, dynamic> toJson() =>
      {'tick': tick, 'string': string, 'fret': fret};
}

class PracticeTab {
  const PracticeTab(
      {required this.id,
      required this.name,
      this.bars = 4,
      this.tuning = 'Standard',
      this.notes = const []});
  final String id;
  final String name;
  final int bars;
  final String tuning;
  final List<PracticeTabNote> notes;

  factory PracticeTab.fromJson(Map<String, dynamic> json) {
    final tab = PracticeTab(
        id: json['id'] as String,
        name: json['name'] as String,
        bars: json['bars'] as int,
        tuning: json['tuning'] as String,
        notes: (json['notes'] as List)
            .map((n) => PracticeTabNote(
                n['tick'] as int, n['string'] as int, n['fret'] as int))
            .toList());
    if (tab.bars < 1 ||
        tab.bars > 64 ||
        tab.name.trim().isEmpty ||
        !_tunings.any((t) => t.name == tab.tuning) ||
        tab.notes.map((n) => n.tick).toSet().length != tab.notes.length ||
        tab.notes.any((n) =>
            n.tick < 0 ||
            n.tick >= tab.bars * 16 ||
            n.string < 1 ||
            n.string > 6 ||
            n.fret < 0 ||
            n.fret > 24)) {
      throw const FormatException('Invalid practice tab');
    }
    return tab;
  }

  Map<String, dynamic> toJson() => {
        'id': id,
        'name': name,
        'bars': bars,
        'tuning': tuning,
        'notes': notes.map((n) => n.toJson()).toList()
      };

  DailyLesson lesson(int bpm) {
    final strings = _tunings.firstWhere((t) => t.name == tuning).notes;
    final sorted = [...notes]..sort((a, b) => a.tick.compareTo(b.tick));
    const names = [
      'C',
      'C#',
      'D',
      'D#',
      'E',
      'F',
      'F#',
      'G',
      'G#',
      'A',
      'A#',
      'B'
    ];
    return DailyLesson(
        title: name,
        subtitle: '$tuning // 4/4 // $bars BARS',
        coaching: 'Play each note as it reaches the fixed marker.',
        bpm: bpm,
        lengthBeats: bars * 4.0,
        stringLabels:
            strings.reversed.map((n) => n.substring(0, n.length - 1)).toList(),
        events: sorted.map((n) {
          final midi = (69 +
                      12 *
                          math.log(
                              _noteFrequency(strings[6 - n.string]) / 440) /
                          math.ln2)
                  .round() +
              n.fret;
          return DailyLessonEvent(
              '${names[midi % 12]}${midi ~/ 12 - 1}', '${n.string} / ${n.fret}',
              beat: n.tick / 4);
        }).toList());
  }
}

class PracticeTabStore {
  PracticeTabStore({String? path})
      : path = path ??
            '${File(PracticeHistoryStore._defaultPath()).parent.path}/practice-tabs.json';
  final String path;

  Future<List<PracticeTab>> load() async {
    final file = File(path);
    if (!await file.exists()) return [];
    return (jsonDecode(await file.readAsString()) as List)
        .map((j) => PracticeTab.fromJson(j as Map<String, dynamic>))
        .toList();
  }

  Future<void> save(List<PracticeTab> tabs) async {
    final file = File(path);
    await file.parent.create(recursive: true);
    final temp = File('$path.new');
    await temp.writeAsString(jsonEncode(tabs.map((t) => t.toJson()).toList()),
        flush: true);
    await temp.rename(path);
  }
}

class PracticeTabLibrary extends StatefulWidget {
  const PracticeTabLibrary(
      {required this.meters, this.onMetronomeChanged, this.store, super.key});
  final Stream<MeterSnapshot> meters;
  final Future<void> Function(bool, int)? onMetronomeChanged;
  final PracticeTabStore? store;
  @override
  State<PracticeTabLibrary> createState() => _PracticeTabLibraryState();
}

class _PracticeTabLibraryState extends State<PracticeTabLibrary> {
  late final _store = widget.store ?? PracticeTabStore();
  List<PracticeTab>? _tabs;
  String? _error;
  bool _busy = false;
  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final tabs = await _store.load();
      if (mounted) {
        setState(() {
          _tabs = tabs;
          _error = null;
        });
      }
    } catch (e) {
      if (mounted) setState(() => _error = 'Could not load tabs: $e');
    }
  }

  Future<void> _edit([PracticeTab? tab]) async {
    await showAeroControlBay<PracticeTab>(context,
        title: tab == null ? 'New practice tab' : 'Edit practice tab',
        icon: Icons.edit_note,
        heightFactor: .94,
        child: PracticeTabEditor(
            onSave: (result) async {
              final tabs = [..._tabs!.where((t) => t.id != result.id), result];
              await _store.save(tabs);
              if (mounted) {
                setState(() {
                  _tabs = tabs;
                  _error = null;
                });
              }
            },
            tab: tab ??
                PracticeTab(
                    id: DateTime.now().microsecondsSinceEpoch.toString(),
                    name: '')));
  }

  Future<void> _persist(List<PracticeTab> tabs) async {
    setState(() => _busy = true);
    try {
      await _store.save(tabs);
      if (mounted) {
        setState(() {
          _tabs = tabs;
          _error = null;
        });
      }
    } catch (e) {
      if (mounted) {
        setState(() =>
            _error = 'Could not save tabs: $e. Your library was not changed.');
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _play(PracticeTab tab) async {
    final settings = await showDialog<(int, int)>(
        context: context, builder: (_) => const _PracticeRunSettings());
    if (settings == null || !mounted) return;
    await showAeroControlBay<void>(context,
        title: 'Practice',
        icon: Icons.music_note,
        heightFactor: .92,
        child: DailyLessonSheet(
            meters: widget.meters,
            lesson: tab.lesson(settings.$1),
            repeatCount: settings.$2,
            customPractice: true,
            onMetronomeChanged: widget.onMetronomeChanged));
  }

  @override
  Widget build(BuildContext context) => Padding(
      padding: const EdgeInsets.all(16),
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        FilledButton.icon(
            onPressed: _tabs == null || _busy ? null : () => _edit(),
            icon: const Icon(Icons.add),
            label: const Text('NEW PRACTICE TAB')),
        const SizedBox(height: 8),
        const Text(
            'Saved on this device • choose BPM and repeats when you play • no history entries'),
        if (_error != null)
          Text(_error!, style: const TextStyle(color: Color(0xffffb3bf))),
        if (_tabs == null && _error != null)
          TextButton(onPressed: _load, child: const Text('Retry')),
        Expanded(
            child: _tabs == null
                ? (_error == null
                    ? const Center(child: CircularProgressIndicator())
                    : const SizedBox())
                : _tabs!.isEmpty
                    ? const Center(
                        child: Text(
                            'Your practice library is empty. Create your first tab.'))
                    : ListView(children: [
                        for (final tab in _tabs!)
                          Card(
                              child: Padding(
                                  padding: const EdgeInsets.all(12),
                                  child: Column(
                                      crossAxisAlignment:
                                          CrossAxisAlignment.start,
                                      children: [
                                        Text(tab.name,
                                            style: const TextStyle(
                                                fontSize: 21,
                                                fontWeight: FontWeight.w800)),
                                        Text(
                                            '${tab.tuning} • ${tab.bars} bars • ${tab.notes.length} notes'),
                                        Wrap(spacing: 8, children: [
                                          FilledButton.icon(
                                              onPressed:
                                                  _busy || tab.notes.isEmpty
                                                      ? null
                                                      : () => _play(tab),
                                              icon:
                                                  const Icon(Icons.play_arrow),
                                              label: const Text('Practice')),
                                          OutlinedButton.icon(
                                              onPressed: _busy
                                                  ? null
                                                  : () => _edit(tab),
                                              icon: const Icon(Icons.edit),
                                              label: const Text('Edit')),
                                          IconButton(
                                              tooltip: 'Delete ${tab.name}',
                                              icon: const Icon(
                                                  Icons.delete_outline),
                                              onPressed: _busy
                                                  ? null
                                                  : () async {
                                                      final yes = await showDialog<
                                                              bool>(
                                                          context: context,
                                                          builder: (c) =>
                                                              AlertDialog(
                                                                  title: Text(
                                                                      'Delete ${tab.name}?'),
                                                                  content:
                                                                      const Text(
                                                                          'This removes the saved practice tab.'),
                                                                  actions: [
                                                                    TextButton(
                                                                        onPressed: () => Navigator.pop(
                                                                            c,
                                                                            false),
                                                                        child: const Text(
                                                                            'Cancel')),
                                                                    FilledButton(
                                                                        onPressed: () => Navigator.pop(
                                                                            c,
                                                                            true),
                                                                        child: const Text(
                                                                            'Delete'))
                                                                  ]));
                                                      if (yes == true &&
                                                          mounted) {
                                                        await _persist(_tabs!
                                                            .where((t) =>
                                                                t.id != tab.id)
                                                            .toList());
                                                      }
                                                    }),
                                        ]),
                                      ])))
                      ])),
      ]));
}

class _PracticeRunSettings extends StatefulWidget {
  const _PracticeRunSettings();
  @override
  State<_PracticeRunSettings> createState() => _PracticeRunSettingsState();
}

class _PracticeRunSettingsState extends State<_PracticeRunSettings> {
  final _form = GlobalKey<FormState>();
  int _bpm = 80, _repeats = 2;
  @override
  Widget build(BuildContext context) => AlertDialog(
          title: const Text('Practice setup'),
          content: Form(
              key: _form,
              child: Column(mainAxisSize: MainAxisSize.min, children: [
                for (final tempo in [true, false])
                  TextFormField(
                      initialValue: tempo ? '80' : '2',
                      decoration: InputDecoration(
                          labelText: tempo ? 'BPM (30–300)' : 'Repeats (1–32)'),
                      keyboardType: TextInputType.number,
                      validator: (v) {
                        final n = int.tryParse(v ?? '');
                        if (n == null ||
                            n < (tempo ? 30 : 1) ||
                            n > (tempo ? 300 : 32)) {
                          return 'Enter a valid whole number';
                        }
                        return null;
                      },
                      onSaved: (v) {
                        if (tempo) {
                          _bpm = int.parse(v!);
                        } else {
                          _repeats = int.parse(v!);
                        }
                      }),
                const SizedBox(height: 12),
                const Text('One four-beat count-in, then continuous repeats.'),
              ])),
          actions: [
            TextButton(
                onPressed: () => Navigator.pop(context),
                child: const Text('Cancel')),
            FilledButton(
                onPressed: () {
                  if (_form.currentState!.validate()) {
                    _form.currentState!.save();
                    Navigator.pop(context, (_bpm, _repeats));
                  }
                },
                child: const Text('Open practice'))
          ]);
}

class PracticeTabEditor extends StatefulWidget {
  const PracticeTabEditor({required this.tab, this.onSave, super.key});
  final PracticeTab tab;
  final Future<void> Function(PracticeTab)? onSave;
  @override
  State<PracticeTabEditor> createState() => _PracticeTabEditorState();
}

class _PracticeTabEditorState extends State<PracticeTabEditor> {
  late final _name = TextEditingController(text: widget.tab.name);
  late int _bars = widget.tab.bars;
  late String _tuning = widget.tab.tuning;
  late final List<PracticeTabNote> _notes = [...widget.tab.notes];
  int _grid = 4;
  bool _dirty = false, _allowClose = false;
  bool _saving = false;
  String? _error;
  final _scroll = ScrollController();
  @override
  void dispose() {
    _name.dispose();
    _scroll.dispose();
    super.dispose();
  }

  Future<bool> _confirm(String title, String message) async =>
      await showDialog<bool>(
          context: context,
          builder: (c) =>
              AlertDialog(title: Text(title), content: Text(message), actions: [
                TextButton(
                    onPressed: () => Navigator.pop(c, false),
                    child: const Text('Cancel')),
                FilledButton(
                    onPressed: () => Navigator.pop(c, true),
                    child: const Text('Continue'))
              ])) ??
      false;

  Future<void> _note(int tick, int string) async {
    final atTick = _notes.where((n) => n.tick == tick).firstOrNull;
    final result = await showDialog<int>(
        context: context,
        builder: (_) => _PracticeFretDialog(note: atTick, string: string));
    if (result == null || !mounted) return;
    setState(() {
      _notes.removeWhere((n) => n.tick == tick);
      if (result >= 0) _notes.add(PracticeTabNote(tick, string, result));
      _dirty = true;
    });
  }

  @override
  Widget build(BuildContext context) {
    final strings =
        _tunings.firstWhere((t) => t.name == _tuning).notes.reversed.toList();
    // Include off-grid notes so switching to a coarser grid never hides data.
    final ticks = {
      for (int t = 0; t < _bars * 16; t += _grid) t,
      ..._notes.map((n) => n.tick)
    }.toList()
      ..sort();
    return PopScope(
        canPop: !_saving && (!_dirty || _allowClose),
        onPopInvokedWithResult: (didPop, _) async {
          if (didPop || !_dirty || _saving) return;
          if (await _confirm('Discard unsaved changes?',
                  'Save the tab to keep your edits.') &&
              mounted) {
            setState(() => _allowClose = true);
            WidgetsBinding.instance.addPostFrameCallback((_) {
              if (mounted) Navigator.pop(context);
            });
          }
        },
        child: SingleChildScrollView(
            padding: const EdgeInsets.all(16),
            child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  TextField(
                      controller: _name,
                      maxLength: 80,
                      decoration:
                          const InputDecoration(labelText: 'Practice tab name'),
                      onChanged: (_) => setState(() => _dirty = true)),
                  Wrap(
                      spacing: 12,
                      runSpacing: 8,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        DropdownButton<String>(
                            value: _tuning,
                            items: [
                              for (final t in _tunings)
                                DropdownMenuItem(
                                    value: t.name, child: Text(t.name))
                            ],
                            onChanged: (v) {
                              if (v != null) {
                                setState(() {
                                  _tuning = v;
                                  _dirty = true;
                                });
                              }
                            }),
                        Text('4/4 • $_bars bars'),
                        OutlinedButton.icon(
                            onPressed: _bars >= 64
                                ? null
                                : () => setState(() {
                                      _bars++;
                                      _dirty = true;
                                    }),
                            icon: const Icon(Icons.add),
                            label: const Text('Add bar')),
                        OutlinedButton.icon(
                            onPressed: _bars <= 1
                                ? null
                                : () async {
                                    final hasNotes = _notes
                                        .any((n) => n.tick >= (_bars - 1) * 16);
                                    if (hasNotes &&
                                        !await _confirm('Remove last bar?',
                                            'Notes in the last bar will be removed.')) {
                                      return;
                                    }
                                    if (mounted) {
                                      setState(() {
                                        _bars--;
                                        _notes.removeWhere(
                                            (n) => n.tick >= _bars * 16);
                                        _dirty = true;
                                      });
                                    }
                                  },
                            icon: const Icon(Icons.remove),
                            label: const Text('Remove bar')),
                      ]),
                  const SizedBox(height: 8),
                  const Text('PLACEMENT GRID',
                      style: TextStyle(
                          color: LostChrome.ice, fontWeight: FontWeight.w900)),
                  Wrap(spacing: 6, children: [
                    for (final entry in {
                      16: 'Whole',
                      8: 'Half',
                      4: 'Quarter',
                      2: 'Eighth',
                      1: 'Sixteenth'
                    }.entries)
                      ChoiceChip(
                          label: Text(entry.value),
                          selected: _grid == entry.key,
                          onSelected: (_) => setState(() => _grid = entry.key))
                  ]),
                  const SizedBox(height: 8),
                  const Text(
                      'Tap a string at a time position, then enter a fret (0–24). One note per position; adding another replaces it. Empty positions are rests.'),
                  const SizedBox(height: 12),
                  Container(
                      decoration: BoxDecoration(
                          color: LostChrome.midnight,
                          borderRadius: BorderRadius.circular(14),
                          border: Border.all(color: LostChrome.ice)),
                      clipBehavior: Clip.antiAlias,
                      child: Row(children: [
                        Column(children: [
                          const SizedBox(height: 40),
                          for (final label in strings)
                            SizedBox(
                                width: 38,
                                height: 48,
                                child: Center(
                                    child: Text(label,
                                        style: const TextStyle(
                                            color: LostChrome.ice))))
                        ]),
                        Expanded(
                            child: Scrollbar(
                                controller: _scroll,
                                thumbVisibility: true,
                                child: SingleChildScrollView(
                                    controller: _scroll,
                                    scrollDirection: Axis.horizontal,
                                    child: Row(children: [
                                      for (final tick in ticks)
                                        SizedBox(
                                            width: 52,
                                            child: Column(children: [
                                              SizedBox(
                                                  height: 40,
                                                  child: Center(
                                                      child: Text(
                                                          '${tick ~/ 16 + 1}:${(tick % 16 / 4 + 1).toStringAsFixed(tick % 4 == 0 ? 0 : 2)}',
                                                          style: TextStyle(
                                                              color: tick %
                                                                          16 ==
                                                                      0
                                                                  ? LostChrome
                                                                      .lime
                                                                  : LostChrome
                                                                      .ice,
                                                              fontSize: 11)))),
                                              for (int string = 1;
                                                  string <= 6;
                                                  string++)
                                                SizedBox(
                                                    height: 48,
                                                    child: Semantics(
                                                        label:
                                                            'Bar ${tick ~/ 16 + 1}, tick ${tick % 16}, string $string',
                                                        button: true,
                                                        child: InkWell(
                                                            onTap: () => _note(
                                                                tick, string),
                                                            child: Container(
                                                                alignment: Alignment
                                                                    .center,
                                                                decoration: BoxDecoration(
                                                                    border: Border(
                                                                        left: BorderSide(
                                                                            color: tick % 16 == 0
                                                                                ? LostChrome
                                                                                    .ice
                                                                                : Colors
                                                                                    .transparent))),
                                                                child: Stack(
                                                                    alignment:
                                                                        Alignment
                                                                            .center,
                                                                    children: [
                                                                      Divider(
                                                                          color: LostChrome
                                                                              .ice
                                                                              .withValues(alpha: .4)),
                                                                      if (_notes.any((n) =>
                                                                          n.tick ==
                                                                              tick &&
                                                                          n.string ==
                                                                              string))
                                                                        Container(
                                                                            color:
                                                                                LostChrome.midnight,
                                                                            padding: const EdgeInsets.symmetric(horizontal: 5),
                                                                            child: Text('${_notes.firstWhere((n) => n.tick == tick && n.string == string).fret}', style: const TextStyle(fontSize: 20, color: LostChrome.pearl, fontWeight: FontWeight.w800))),
                                                                    ]))))),
                                            ]))
                                    ])))),
                      ])),
                  const SizedBox(height: 16),
                  if (_error != null)
                    Text(_error!,
                        style: const TextStyle(color: Color(0xffffb3bf))),
                  FilledButton.icon(
                      icon: const Icon(Icons.save),
                      label: const Text('SAVE TAB'),
                      onPressed: _saving
                          ? null
                          : () async {
                              if (_name.text.trim().isEmpty) {
                                setState(
                                    () => _error = 'Give your tab a name.');
                                return;
                              }
                              final result = PracticeTab(
                                  id: widget.tab.id,
                                  name: _name.text.trim(),
                                  bars: _bars,
                                  tuning: _tuning,
                                  notes: [..._notes]
                                    ..sort((a, b) => a.tick.compareTo(b.tick)));
                              setState(() => _saving = true);
                              try {
                                await widget.onSave?.call(result);
                              } catch (e) {
                                if (mounted) {
                                  setState(() {
                                    _saving = false;
                                    _error =
                                        'Could not save: $e. Your edits are still here.';
                                  });
                                }
                                return;
                              }
                              if (!mounted) return;
                              setState(() {
                                _saving = false;
                                _allowClose = true;
                              });
                              WidgetsBinding.instance.addPostFrameCallback((_) {
                                if (mounted) Navigator.pop(context, result);
                              });
                            }),
                ])));
  }
}

class _PracticeFretDialog extends StatefulWidget {
  const _PracticeFretDialog({required this.note, required this.string});
  final PracticeTabNote? note;
  final int string;
  @override
  State<_PracticeFretDialog> createState() => _PracticeFretDialogState();
}

class _PracticeFretDialogState extends State<_PracticeFretDialog> {
  final _form = GlobalKey<FormState>();
  late final _fret = TextEditingController(text: '${widget.note?.fret ?? 0}');
  @override
  void dispose() {
    _fret.dispose();
    super.dispose();
  }

  void _save() {
    if (_form.currentState!.validate()) {
      Navigator.pop(context, int.parse(_fret.text));
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
          title: Text('String ${widget.string}'),
          content: Form(
              key: _form,
              child: TextFormField(
                  controller: _fret,
                  autofocus: true,
                  keyboardType: TextInputType.number,
                  decoration: const InputDecoration(labelText: 'Fret (0–24)'),
                  onFieldSubmitted: (_) => _save(),
                  validator: (v) {
                    final n = int.tryParse(v ?? '');
                    return n == null || n < 0 || n > 24
                        ? 'Choose a fret from 0 to 24'
                        : null;
                  })),
          actions: [
            if (widget.note != null)
              TextButton(
                  onPressed: () => Navigator.pop(context, -1),
                  child: const Text('Delete note')),
            TextButton(
                onPressed: () => Navigator.pop(context),
                child: const Text('Cancel')),
            FilledButton(onPressed: _save, child: const Text('Set note'))
          ]);
}
