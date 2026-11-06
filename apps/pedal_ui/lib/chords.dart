part of 'main.dart';

const chordRoots = [
  'C',
  'C#',
  'D',
  'Eb',
  'E',
  'F',
  'F#',
  'G',
  'Ab',
  'A',
  'Bb',
  'B'
];

class ChordDefinition {
  ChordDefinition(Map<String, dynamic> data)
      : id = data['id'] as String,
        label = data['label'] as String,
        notes = List<String>.from(data['notes']),
        shapes = (data['shapes'] as List)
            .map((s) => Map<String, dynamic>.from(s))
            .toList();
  final String id, label;
  final List<String> notes;
  final List<Map<String, dynamic>> shapes;
}

class ChordProgressionStore {
  ChordProgressionStore({String? path})
      : path = path ??
            '${File(PracticeHistoryStore._defaultPath()).parent.path}/chord-progressions.json';
  final String path;
  Future<List<Map<String, dynamic>>> load() async {
    final file = File(path);
    if (!await file.exists()) return [];
    return (jsonDecode(await file.readAsString()) as List)
        .map((v) => Map<String, dynamic>.from(v))
        .toList();
  }

  Future<void> save(List<Map<String, dynamic>> items) async {
    final file = File(path);
    await file.parent.create(recursive: true);
    final temp = File('$path.new');
    await temp.writeAsString(jsonEncode(items), flush: true);
    await temp.rename(path);
  }
}

class ChordWorkspace extends StatefulWidget {
  const ChordWorkspace(
      {required this.catalog,
      required this.client,
      required this.active,
      this.store,
      super.key});
  final CatalogClient catalog;
  final ControlClient client;
  final bool active;
  final ChordProgressionStore? store;
  @override
  State<ChordWorkspace> createState() => _ChordWorkspaceState();
}

class _ChordWorkspaceState extends State<ChordWorkspace> {
  late final _store = widget.store ?? ChordProgressionStore();
  final _name = TextEditingController(text: 'New progression');
  final _prompt = TextEditingController();
  final Map<String, ChordDefinition> _chords = {};
  List<Map<String, dynamic>> _saved = [], _suggestions = [], _steps = [];
  String _key = 'C',
      _chordId = 'C:major',
      _description = '',
      _source = 'Custom';
  String? _id, _error;
  String _voicing = 'comfortable';
  String _mode = 'major', _section = 'Verse', _pattern = 'up';
  int _division = 2;
  Map<String, dynamic>? _phrase;
  String? _phraseSource;
  String get _phraseIdentity => jsonEncode([_steps, _key, _mode]);
  bool _selecting = false;
  bool _variationDialogOpen = false;
  final Set<int> _variationSelection = {};
  List<Map<String, dynamic>>? _variationUndo, _variationApplied, _auditionSteps;
  int _shape = 0, _bpm = 90, _generation = 0, _playStep = -1;
  int? _selected;
  bool _loaded = false, _loading = false, _ai = false, _busy = false;
  bool _dirty = false,
      _storageReady = false,
      _loop = true,
      _playing = false,
      _owns = false;
  Timer? _poll;
  bool _polling = false;
  bool _previewPending = false;
  bool get _locked => _busy || _playing;

  @override
  void initState() {
    super.initState();
    if (widget.active) _load();
  }

  @override
  void didUpdateWidget(covariant ChordWorkspace oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.active && !_loaded && !_loading) _load();
    if (!widget.active && oldWidget.active) unawaited(_stop());
  }

  @override
  void dispose() {
    _generation++;
    _poll?.cancel();
    _name.dispose();
    _prompt.dispose();
    if (_owns) {
      widget.client.timelineCommand(
          'timeline_stop', {}).catchError((Object _) => <String, dynamic>{});
    }
    super.dispose();
  }

  List<Map<String, dynamic>> _validateSteps(dynamic value) {
    if (value is! List || value.isEmpty || value.length > 16) {
      throw const FormatException('Use 1–16 chords');
    }
    return value.map((v) {
      final step = Map<String, dynamic>.from(v);
      final chord = _chords[step['chord']];
      final shape = step['shape'], beats = step['beats'];
      if (chord == null ||
          shape is! int ||
          shape < 0 ||
          shape >= chord.shapes.length ||
          beats is! int ||
          beats < 1 ||
          beats > 8) {
        throw const FormatException('Invalid chord or shape');
      }
      if ((step.containsKey('pattern') &&
              !phrasePatterns.containsKey(step['pattern'])) ||
          (step.containsKey('division') &&
              ![1, 2, 4].contains(step['division']))) {
        throw const FormatException('Invalid phrase pattern');
      }
      return step;
    }).toList();
  }

  Future<void> _load() async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final data = await widget.catalog.chordCommand('library', {});
      if (!mounted) return;
      for (final item in data['chords'] as List) {
        final chord = ChordDefinition(Map<String, dynamic>.from(item));
        _chords[chord.id] = chord;
      }
      _ai = data['ai_available'] == true;
      _loaded = true;
      try {
        final saved = await _store.load();
        for (final p in saved) {
          _validateSteps(p['steps']);
          if (p['id'] is! String ||
              p['name'] is! String ||
              !chordRoots.contains(p['key']) ||
              p['bpm'] is! int ||
              p['bpm'] < 40 ||
              p['bpm'] > 240) {
            throw const FormatException('Invalid saved progression');
          }
        }
        if (mounted) {
          setState(() {
            _saved = saved;
            _storageReady = true;
          });
        }
      } catch (e) {
        if (mounted) {
          setState(() => _error =
              'Saved library could not be read; saving is disabled to protect it: $e');
        }
      }
    } catch (e) {
      if (mounted) {
        setState(() =>
            _error = 'Could not load chords. Start or restart the catalog. $e');
      }
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  Future<void> _suggest(bool ai) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final result = await widget.catalog.chordCommand('suggest',
          {'key': _key, 'prompt': _prompt.text, 'ai': ai, 'voicing': _voicing});
      final options = (result['progressions'] as List)
          .map((v) => Map<String, dynamic>.from(v))
          .toList();
      for (final p in options) {
        _validateSteps(p['steps']);
      }
      if (mounted) setState(() => _suggestions = options);
    } catch (e) {
      if (mounted) {
        setState(() =>
            _error = 'Suggestion failed: $e. Offline ideas remain available.');
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
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

  Future<void> _apply(Map<String, dynamic> p) async {
    if (_dirty &&
        !await _confirm('Replace unsaved progression?',
            'Save it first if you want to keep your changes.')) {
      return;
    }
    if (!mounted) return;
    try {
      final steps = _validateSteps(p['steps']);
      setState(() {
        _steps = steps;
        _variationSelection.clear();
        _variationUndo = null;
        _name.text = p['name'] as String;
        _key = p['key'] as String;
        _mode = phraseModes.contains(p['mode'])
            ? p['mode'] as String
            : 'unspecified';
        _section = phraseSections.contains(p['section'])
            ? p['section'] as String
            : 'Custom';
        _id = p['id'] as String?;
        _bpm = p['bpm'] as int? ?? _bpm;
        _description = p['description'] as String? ?? '';
        _source = p['source'] as String? ?? 'Saved';
        _selected = 0;
        _chordId = steps.first['chord'];
        _shape = steps.first['shape'];
        _dirty = _id == null;
      });
    } catch (e) {
      setState(() => _error = '$e');
    }
  }

  Future<void> _save() async {
    if (_steps.isEmpty || _name.text.trim().isEmpty) {
      setState(() =>
          _error = 'Give the progression a name and add at least one chord.');
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    final id = _id ?? DateTime.now().microsecondsSinceEpoch.toString();
    final record = <String, dynamic>{
      'id': id,
      'name': _name.text.trim(),
      'key': _key,
      'bpm': _bpm,
      'description': _description,
      'source': _source,
      'steps': _steps.map((s) => Map<String, dynamic>.from(s)).toList(),
      'phrase_version': 1,
      'mode': _mode,
      'section': _section
    };
    final updated = [..._saved.where((p) => p['id'] != id), record];
    try {
      await _store.save(updated);
      if (mounted) {
        setState(() {
          _saved = updated;
          _id = id;
          _dirty = false;
        });
      }
    } catch (e) {
      if (mounted) {
        setState(
            () => _error = 'Could not save; your edits are still here. $e');
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  void _editChord(String id, int shape) => setState(() {
        _chordId = id;
        _shape = shape;
        if (_selected != null) {
          _steps[_selected!] = {
            ..._steps[_selected!],
            'chord': id,
            'shape': shape
          };
          _dirty = true;
        }
      });

  void _transpose(String key) => setState(() {
        _variationSelection.clear();
        final delta = chordRoots.indexOf(key) - chordRoots.indexOf(_key);
        _steps = _steps.map((s) {
          final parts = (s['chord'] as String).split(':');
          return {
            ...s,
            'chord':
                '${chordRoots[(chordRoots.indexOf(parts[0]) + delta) % 12]}:${parts[1]}',
            'shape': 0
          };
        }).toList();
        _key = key;
        _selected = null;
        _dirty = true;
        _suggestions = [];
      });

  Future<void> _buildPhrase() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    final identity = _phraseIdentity;
    try {
      final result = await widget.catalog.chordCommand(
          'phrase', {'steps': _steps, 'key': _key, 'mode': _mode});
      if (mounted) {
        setState(() {
          _phrase = result;
          _phraseSource = identity;
        });
      }
    } catch (e) {
      if (mounted) setState(() => _error = 'Could not build phrase: $e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Widget _phraseTools() => _panel(
      'PHRASE / SECTION SKETCH',
      Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        const Text(
            'Keep the harmony; choose how each chord is played. These saved sketches can become song sections later.'),
        Wrap(
            spacing: 12,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              const Text('Section'),
              DropdownButton<String>(
                  value: _section,
                  items: [
                    for (final v in phraseSections)
                      DropdownMenuItem(value: v, child: Text(v))
                  ],
                  onChanged: _locked
                      ? null
                      : (v) => setState(() {
                            _section = v!;
                            _dirty = true;
                          })),
              Text('Key: $_key'),
              DropdownButton<String>(
                  value: _mode,
                  items: [
                    for (final v in phraseModes)
                      DropdownMenuItem(
                          value: v,
                          child:
                              Text(v == 'unspecified' ? 'Mode unspecified' : v))
                  ],
                  onChanged: _locked
                      ? null
                      : (v) => setState(() {
                            _mode = v!;
                            _dirty = true;
                          })),
            ]),
        DropdownButtonFormField<String>(
            initialValue: _pattern,
            isExpanded: true,
            decoration: const InputDecoration(labelText: 'Performance pattern'),
            items: [
              for (final p in phrasePatterns.entries)
                DropdownMenuItem(value: p.key, child: Text(p.value))
            ],
            onChanged: _locked ? null : (v) => setState(() => _pattern = v!)),
        Wrap(
            spacing: 10,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              const Text('Placement'),
              DropdownButton<int>(
                  value: _division,
                  items: const [
                    DropdownMenuItem(value: 1, child: Text('Quarter notes')),
                    DropdownMenuItem(value: 2, child: Text('Eighth notes')),
                    DropdownMenuItem(value: 4, child: Text('Sixteenth notes')),
                  ],
                  onChanged:
                      _locked ? null : (v) => setState(() => _division = v!)),
              FilledButton(
                  onPressed: _locked ||
                          _steps.isEmpty ||
                          (_selecting && _variationSelection.isEmpty)
                      ? null
                      : () {
                          setState(() {
                            for (var i = 0; i < _steps.length; i++) {
                              if (_selecting
                                  ? _variationSelection.contains(i)
                                  : (_selected == null || _selected == i)) {
                                _steps[i] = {
                                  ..._steps[i],
                                  'pattern': _pattern,
                                  'division': _division
                                };
                              }
                            }
                            _dirty = true;
                          });
                          _buildPhrase();
                        },
                  child: Text(_selecting
                      ? 'Apply to marked chords'
                      : _selected == null
                          ? 'Apply to all chords'
                          : 'Apply to chord ${_selected! + 1}')),
            ]),
        for (var i = 0; i < _steps.length; i++)
          Text(
              '${i + 1}. ${_chords[_steps[i]['chord']]!.label} · ${phrasePatterns[_steps[i]['pattern'] ?? 'block']} · ${_steps[i]['beats']} beats'),
        const SizedBox(height: 8),
        Wrap(spacing: 8, runSpacing: 8, children: [
          OutlinedButton(
              onPressed: _locked || _steps.isEmpty ? null : _buildPhrase,
              child: const Text('Show phrase tab')),
          FilledButton.icon(
              onPressed: _playing || _previewPending
                  ? _stop
                  : _locked || _steps.isEmpty
                      ? null
                      : () => _play(performance: true),
              icon: Icon(_playing ? Icons.stop : Icons.play_arrow),
              label: Text(_playing ? 'Stop phrase' : 'Play phrase')),
        ]),
        if (_phrase != null && _phraseSource == _phraseIdentity) ...[
          if ((_phrase!['outside_key'] as List).isNotEmpty)
            Text(
                'Outside selected key: ${(_phrase!['outside_key'] as List).join(', ')}. Kept as intentional color; not removed.',
                style: const TextStyle(color: LostChrome.lime)),
          GuitarPhraseTab(_phrase!),
        ] else if (_phrase != null)
          const Text('Phrase changed — show the tab or play to update it.'),
        const Text(
            'Tab bars show actual note lengths; gaps are rests. Offbeats skip alternating placements. Muted bass is a synthesized approximation. Save progression keeps patterns and section context.',
            style: TextStyle(color: LostChrome.ice, fontSize: 12)),
      ]));

  Future<void> _vary(String action, String title) async {
    setState(() => _variationDialogOpen = true);
    final original = _steps.map((s) => Map<String, dynamic>.from(s)).toList();
    final selected = _variationSelection.toList()..sort();
    List<Map<String, dynamic>>? candidate;
    String? problem;
    var attempt = 0, pending = false, open = true, started = false;
    Future<void> request(StateSetter refresh) async {
      refresh(() {
        pending = true;
        problem = null;
      });
      await _stop();
      try {
        final result = await widget.catalog.chordCommand('vary', {
          'steps': original,
          'selected': selected,
          'action': action,
          'key': _key,
          'attempt': attempt++,
          'voicing': _voicing,
        });
        final next = _validateSteps(result['steps']);
        if (next.length != original.length ||
            List.generate(
                    next.length,
                    (i) =>
                        next[i]['beats'] != original[i]['beats'] ||
                        (!selected.contains(i) &&
                            jsonEncode(next[i]) != jsonEncode(original[i])))
                .any((v) => v)) {
          throw const FormatException(
              'Variation changed protected chords or timing');
        }
        if (open) {
          refresh(() {
            candidate = next;
            if (jsonEncode(next) == jsonEncode(original)) {
              problem =
                  'No change needed for this option. Try another direction.';
            }
          });
        }
      } catch (e) {
        if (open) {
          refresh(() {
            candidate = null;
            problem = 'Could not make variation: $e';
          });
        }
      } finally {
        if (open) refresh(() => pending = false);
      }
    }

    final result = await showAeroControlBay<List<Map<String, dynamic>>>(context,
        title: title.toUpperCase(), icon: Icons.auto_fix_high,
        child: StatefulBuilder(builder: (dialogContext, refresh) {
      if (!started) {
        started = true;
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (open) request(refresh);
        });
      }
      String summary(List<Map<String, dynamic>> steps) =>
          steps.asMap().entries.map((entry) {
            final s = entry.value;
            return '${selected.contains(entry.key) ? '● ' : ''}${_chords[s['chord']]!.label} (${s['beats']} beats)';
          }).join(' → ');
      Future<void> hear(List<Map<String, dynamic>> steps) async {
        refresh(() => pending = true);
        await _stop();
        if (!open) return;
        await _play(
            audition: steps,
            performance: steps
                .any((s) => s['pattern'] != null && s['pattern'] != 'block'));
        if (open) {
          refresh(() {
            pending = false;
            problem = _error;
          });
        }
      }

      return Semantics(
          container: true,
          explicitChildNodes: true,
          child: SingleChildScrollView(
              padding: const EdgeInsets.all(16),
              child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    const Text(
                        'Only marked chords change. Timing stays the same. Offline suggestions—not fixed rules for mood.'),
                    const SizedBox(height: 12),
                    _panel('ORIGINAL', Text(summary(original))),
                    if (candidate != null)
                      _panel(
                          'VARIATION',
                          Column(
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: [
                                Text(summary(candidate!)),
                                for (final i in selected)
                                  Text(
                                      '${i + 1}. ${_chords[candidate![i]['chord']]!.shapes[candidate![i]['shape']]['name']}'),
                              ])),
                    if (problem != null)
                      Text(problem!,
                          style: const TextStyle(color: LostChrome.ice)),
                    SizedBox(
                        height: 4,
                        child:
                            LinearProgressIndicator(value: pending ? null : 0)),
                    Wrap(spacing: 8, runSpacing: 8, children: [
                      OutlinedButton(
                          onPressed: pending ? null : () => hear(original),
                          child: const Text('Hear original')),
                      OutlinedButton(
                          onPressed: pending || candidate == null
                              ? null
                              : () => hear(candidate!),
                          child: const Text('Hear variation')),
                      TextButton(
                          onPressed: () async {
                            await _stop();
                            if (open) refresh(() {});
                          },
                          child: const Text('Stop')),
                      OutlinedButton(
                          onPressed: pending ? null : () => request(refresh),
                          child: Text(candidate == null
                              ? 'Make variation'
                              : 'Try again')),
                      FilledButton(
                          onPressed: pending ||
                                  candidate == null ||
                                  jsonEncode(candidate) == jsonEncode(original)
                              ? null
                              : () => Navigator.pop(dialogContext, candidate),
                          child: const Text('Apply variation')),
                      TextButton(
                          onPressed: () => Navigator.pop(dialogContext),
                          child: const Text('Cancel')),
                    ]),
                  ])));
    }));
    open = false;
    await _stop();
    if (!mounted) return;
    setState(() => _variationDialogOpen = false);
    if (result == null) return;
    setState(() {
      _variationUndo = original;
      _variationApplied =
          result.map((s) => Map<String, dynamic>.from(s)).toList();
      _steps = result;
      _selected = null;
      _dirty = true;
    });
  }

  void _updatePlayback(VoidCallback update) {
    // The comparison dialog owns its UI while open; do not rebuild the covered
    // scrollable workspace on every transport tick.
    if (_variationDialogOpen) {
      update();
    } else {
      setState(update);
    }
  }

  Future<void> _stop() async {
    _generation++;
    _poll?.cancel();
    if (_owns) {
      try {
        await widget.client.timelineCommand('timeline_stop', {});
      } catch (e) {
        if (mounted) {
          _updatePlayback(() => _error = 'Could not stop preview: $e');
        }
        return;
      }
      _owns = false;
    }
    if (mounted) {
      _updatePlayback(() {
        _playing = false;
        _playStep = -1;
      });
    }
  }

  Future<void> _play(
      {bool single = false,
      List<Map<String, dynamic>>? audition,
      bool performance = false}) async {
    if (_locked) return;
    final steps = single
        ? [
            {'chord': _chordId, 'shape': _shape, 'beats': 4}
          ]
        : audition ?? _steps;
    if (steps.isEmpty) return;
    final generation = ++_generation;
    _updatePlayback(() {
      _busy = true;
      _auditionSteps = steps;
      _previewPending = true;
      _error = null;
    });
    try {
      final status = await widget.client.timelineCommand('timeline_status', {});
      if (status['playing'] == true || status['recording'] == true) {
        throw Exception(
            'Stop Studio playback or recording before previewing chords.');
      }
      final identity = _phraseIdentity;
      final audio = await widget.catalog.chordCommand(
          performance ? 'phrase_preview' : 'preview',
          {'steps': steps, 'bpm': _bpm, 'key': _key, 'mode': _mode});
      if (!mounted || !widget.active || generation != _generation) return;
      if (performance && audition == null && audio['phrase'] is Map) {
        _phrase = Map<String, dynamic>.from(audio['phrase']);
        _phraseSource = identity;
      }
      // Rendering may take time: recheck before taking ownership of the transport.
      final latest = await widget.client.timelineCommand('timeline_status', {});
      if (latest['playing'] == true || latest['recording'] == true) {
        throw Exception('Studio is using the audio transport.');
      }
      if (!mounted || !widget.active || generation != _generation) return;
      _owns = true;
      await widget.client.timelineCommand('timeline_start', {
        'name': 'Chord preview',
        'record': false,
        'count_in_bars': 0,
        'click': false,
        'bpm': _bpm,
        'loop': !single && _loop,
        'start_ms': 0,
        'tracks': [
          {
            'id': 'chord-preview',
            'volume': .8,
            'pan': 0.0,
            'muted': false,
            'solo': false,
            'clips': [
              {
                'path': audio['path'],
                'start_ms': 0,
                'source_in_ms': 0,
                'length_ms': audio['duration_ms']
              }
            ]
          }
        ],
      });
      if (!mounted || !widget.active || generation != _generation) {
        await _stop();
        return;
      }
      _updatePlayback(() => _playing = true);
      _poll = Timer.periodic(const Duration(milliseconds: 80), (_) async {
        if (_polling) return;
        _polling = true;
        try {
          final state =
              await widget.client.timelineCommand('timeline_status', {});
          if (!mounted || generation != _generation) return;
          if (state['playing'] != true) {
            _owns = false;
            await _stop();
            return;
          }
          final beat = (state['position_frames'] as num) /
              (state['sample_rate'] as num) *
              _bpm /
              60;
          var end = 0.0, index = 0;
          for (; index < steps.length; index++) {
            end += steps[index]['beats'] as int;
            if (beat < end) break;
          }
          _updatePlayback(
              () => _playStep = single ? -1 : index.clamp(0, steps.length - 1));
        } catch (e) {
          if (mounted) {
            _updatePlayback(() => _error = 'Preview status unavailable: $e');
          }
          await _stop();
        } finally {
          _polling = false;
        }
      });
    } catch (e) {
      await _stop();
      if (mounted) _updatePlayback(() => _error = 'Preview failed: $e');
    } finally {
      if (mounted) {
        _updatePlayback(() {
          _busy = false;
          _previewPending = false;
        });
      }
    }
  }

  Widget _panel(String title, Widget child) => Container(
      padding: const EdgeInsets.all(16),
      margin: const EdgeInsets.only(bottom: 12),
      decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(20),
          border: Border.all(color: LostChrome.ice.withValues(alpha: .75)),
          gradient: const LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [
                Color(0xff226fa1),
                Color(0xff10365f),
                Color(0xff081b3f)
              ])),
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Text(title,
            style: const TextStyle(
                color: LostChrome.ice,
                fontWeight: FontWeight.w900,
                letterSpacing: 1.2)),
        const SizedBox(height: 12),
        child
      ]));

  @override
  Widget build(BuildContext context) {
    if (!_loaded) {
      return Center(
          child: _loading
              ? const CircularProgressIndicator()
              : Column(mainAxisSize: MainAxisSize.min, children: [
                  Text(_error ?? 'Open Chords to load the local library.'),
                  TextButton(onPressed: _load, child: const Text('Retry'))
                ]));
    }
    final playingStep = _playing && _playStep >= 0
        ? (_auditionSteps ?? _steps)[_playStep]
        : null;
    final chord = _chords[playingStep?['chord'] ?? _chordId]!;
    final shapeIndex = playingStep?['shape'] as int? ?? _shape;
    final shape = chord.shapes[shapeIndex];
    final browser = _panel(
        _selected == null ? 'CHORD ATLAS' : 'EDIT CHORD ${_selected! + 1}',
        Column(children: [
          Wrap(
              spacing: 12,
              runSpacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                DropdownButton<String>(
                    value: _chordId.split(':')[0],
                    items: [
                      for (final root in chordRoots)
                        DropdownMenuItem(value: root, child: Text(root))
                    ],
                    onChanged: _locked
                        ? null
                        : (v) => _editChord('$v:${_chordId.split(':')[1]}', 0)),
                DropdownButton<String>(
                    value: _chordId.split(':')[1],
                    items: [
                      for (final q
                          in _chords.keys.map((id) => id.split(':')[1]).toSet())
                        DropdownMenuItem(
                            value: q, child: Text(q == '5' ? 'Power (5)' : q))
                    ],
                    onChanged: _locked
                        ? null
                        : (v) => _editChord('${_chordId.split(':')[0]}:$v', 0)),
                if (_selected != null)
                  TextButton(
                      onPressed: _locked
                          ? null
                          : () => setState(() => _selected = null),
                      child: const Text('Browse freely')),
              ]),
          Text(chord.label,
              style:
                  const TextStyle(fontSize: 32, fontWeight: FontWeight.w900)),
          Text(chord.notes.join(' • '),
              style: const TextStyle(color: LostChrome.ice)),
          const SizedBox(height: 8),
          Semantics(
              label:
                  '${chord.label}, low E to high e frets: ${(shape['frets'] as List).map((f) => f == -1 ? 'muted' : f).join(', ')}',
              child: SizedBox(
                  width: 280,
                  height: 265,
                  child: CustomPaint(painter: ChordShapePainter(shape)))),
          Text(shape['name'], style: const TextStyle(color: LostChrome.ice)),
          const Text(
              'Standard tuning • low E on left • × muted • ○ open\nNumbers = fingers (1 index → 4 little); repeated fingers form a barre.',
              textAlign: TextAlign.center,
              style: TextStyle(color: LostChrome.ice, fontSize: 12)),
          Wrap(alignment: WrapAlignment.center, spacing: 8, children: [
            IconButton(
                tooltip: 'Previous shape',
                onPressed: _locked
                    ? null
                    : () => _editChord(
                        _chordId, (_shape - 1) % chord.shapes.length),
                icon: const Icon(Icons.chevron_left)),
            TextButton(
                onPressed: _locked
                    ? null
                    : () => _editChord(
                        _chordId, (_shape + 1) % chord.shapes.length),
                child: Text('Shape ${shapeIndex + 1}/${chord.shapes.length}')),
            IconButton(
                tooltip: 'Next shape',
                onPressed: _locked
                    ? null
                    : () => _editChord(
                        _chordId, (_shape + 1) % chord.shapes.length),
                icon: const Icon(Icons.chevron_right)),
          ]),
          Wrap(
              spacing: 8,
              runSpacing: 8,
              alignment: WrapAlignment.center,
              children: [
                OutlinedButton.icon(
                    onPressed: _locked ? null : () => _play(single: true),
                    icon: const Icon(Icons.volume_up),
                    label: const Text('Hear chord')),
                FilledButton.icon(
                    onPressed: _locked || _steps.length >= 16
                        ? null
                        : () => setState(() {
                              _steps.add({
                                'chord': _chordId,
                                'shape': _shape,
                                'beats': 4
                              });
                              _dirty = true;
                            }),
                    icon: const Icon(Icons.add),
                    label: const Text('Add chord')),
              ]),
        ]));
    final builder = Column(children: [
      _panel(
          'PROGRESSION LAB',
          Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
            TextField(
                controller: _name,
                enabled: !_locked,
                maxLength: 80,
                decoration: InputDecoration(
                    labelText: _dirty
                        ? 'Progression name • unsaved'
                        : 'Progression name'),
                onChanged: (_) => setState(() => _dirty = true)),
            Wrap(
                spacing: 10,
                runSpacing: 8,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  const Text('Tonal center'),
                  DropdownButton<String>(
                      value: _key,
                      items: [
                        for (final k in chordRoots)
                          DropdownMenuItem(value: k, child: Text(k))
                      ],
                      onChanged: _locked ? null : (v) => _transpose(v!)),
                  Text('$_bpm BPM'),
                  FilterChip(
                      label: const Text('Loop'),
                      selected: _loop,
                      onSelected:
                          _locked ? null : (v) => setState(() => _loop = v)),
                ]),
            Slider(
                value: _bpm.toDouble(),
                min: 40,
                max: 240,
                divisions: 200,
                label: '$_bpm BPM',
                onChanged: _locked
                    ? null
                    : (v) => setState(() {
                          _bpm = v.round();
                          _dirty = true;
                        })),
            const Text(
                'Changing tonal center transposes the progression. Tap a chord to edit its shape.',
                style: TextStyle(color: LostChrome.ice, fontSize: 12)),
            const SizedBox(height: 8),
            if (_steps.isEmpty)
              const Padding(
                  padding: EdgeInsets.all(20),
                  child: Text(
                      'Add a chord from the atlas or choose a suggestion below.')),
            Wrap(spacing: 8, runSpacing: 8, children: [
              for (var i = 0; i < _steps.length; i++)
                ActionChip(
                    backgroundColor: _playStep == i ? LostChrome.ocean : null,
                    side: BorderSide(
                        color: _selected == i ||
                                _playStep == i ||
                                _variationSelection.contains(i)
                            ? LostChrome.lime
                            : LostChrome.ice),
                    label: Text(
                        '${i + 1}. ${_chords[_steps[i]['chord']]!.label} • ${_steps[i]['beats']} beats'),
                    onPressed: _locked
                        ? null
                        : () => setState(() {
                              if (_selecting) {
                                if (!_variationSelection.add(i)) {
                                  _variationSelection.remove(i);
                                }
                                return;
                              }
                              _selected = i;
                              _chordId = _steps[i]['chord'];
                              _shape = _steps[i]['shape'];
                            }))
            ]),
            Wrap(spacing: 8, runSpacing: 8, children: [
              FilterChip(
                  label: Text(_selecting
                      ? 'Selecting ${_variationSelection.length} chords'
                      : 'Select chords'),
                  selected: _selecting,
                  onSelected: _locked
                      ? null
                      : (v) => setState(() {
                            _selecting = v;
                            _selected = null;
                            _variationSelection.clear();
                          })),
              if (_selecting)
                for (final action in const {
                  'darker': 'Darker',
                  'brighter': 'Brighter',
                  'colorful': 'More colorful',
                  'simpler': 'Simpler',
                  'tension': 'More tension',
                  'resolve': 'More resolved',
                  'smoother': 'Smoother voicings',
                  'voicelead': 'Voice-led transitions'
                }.entries)
                  OutlinedButton(
                      onPressed: _locked || _variationSelection.isEmpty
                          ? null
                          : () => _vary(action.key, action.value),
                      child: Text(action.value)),
              if (_variationUndo != null)
                TextButton.icon(
                    onPressed: _locked ||
                            jsonEncode(_steps) != jsonEncode(_variationApplied)
                        ? null
                        : () => setState(() {
                              _steps = _variationUndo!;
                              _variationUndo = null;
                              _selected = null;
                              _dirty = true;
                            }),
                    icon: const Icon(Icons.undo),
                    label: const Text('Undo variation')),
            ]),
            if (_selected != null)
              Wrap(
                  spacing: 8,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [
                    const Text('Beats'),
                    DropdownButton<int>(
                        value: _steps[_selected!]['beats'],
                        items: [
                          for (int n = 1; n <= 8; n++)
                            DropdownMenuItem(value: n, child: Text('$n'))
                        ],
                        onChanged: _locked
                            ? null
                            : (v) => setState(() {
                                  _steps[_selected!]['beats'] = v;
                                  _dirty = true;
                                })),
                    for (final direction in [-1, 1])
                      IconButton(
                          tooltip: direction < 0
                              ? 'Move chord left'
                              : 'Move chord right',
                          onPressed: _locked ||
                                  _selected! + direction < 0 ||
                                  _selected! + direction >= _steps.length
                              ? null
                              : () => setState(() {
                                    final step = _steps.removeAt(_selected!);
                                    _selected = _selected! + direction;
                                    _steps.insert(_selected!, step);
                                    _dirty = true;
                                  }),
                          icon: Icon(direction < 0
                              ? Icons.arrow_back
                              : Icons.arrow_forward)),
                    IconButton(
                        tooltip: 'Remove chord',
                        onPressed: _locked
                            ? null
                            : () => setState(() {
                                  _steps.removeAt(_selected!);
                                  _selected = null;
                                  _dirty = true;
                                }),
                        icon: const Icon(Icons.delete_outline)),
                  ]),
            const SizedBox(height: 12),
            Wrap(spacing: 8, runSpacing: 8, children: [
              FilledButton.icon(
                  onPressed: _playing || _previewPending
                      ? _stop
                      : _steps.isEmpty || _busy
                          ? null
                          : () => _play(),
                  icon: Icon(_playing || _previewPending
                      ? Icons.stop
                      : Icons.play_arrow),
                  label: Text(_playing || _previewPending
                      ? 'Stop preview'
                      : 'Play progression')),
              OutlinedButton.icon(
                  onPressed: _locked || !_storageReady ? null : _save,
                  icon: const Icon(Icons.save),
                  label: const Text('Save progression')),
              TextButton(
                  onPressed: _locked
                      ? null
                      : () async {
                          if (_dirty &&
                              !await _confirm('Start a new progression?',
                                  'Unsaved changes will be discarded.')) {
                            return;
                          }
                          if (mounted) {
                            setState(() {
                              _steps = [];
                              _variationSelection.clear();
                              _variationUndo = null;
                              _selected = null;
                              _id = null;
                              _name.text = 'New progression';
                              _description = '';
                              _source = 'Custom';
                              _mode = 'major';
                              _section = 'Verse';
                              _dirty = false;
                            });
                          }
                        },
                  child: const Text('New')),
            ]),
            if (_description.isNotEmpty)
              Padding(
                  padding: const EdgeInsets.only(top: 10),
                  child: Text('$_source • $_description',
                      style: const TextStyle(color: LostChrome.ice))),
            const SizedBox(height: 8),
            const Text(
                'Synthesized reference through your configured engine output—not a guitar simulation.',
                style: TextStyle(fontSize: 12, color: LostChrome.ice)),
          ])),
      _phraseTools(),
      _panel(
          'FIND A FEELING',
          Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
            TextField(
                controller: _prompt,
                enabled: !_locked,
                maxLength: 600,
                minLines: 1,
                maxLines: 3,
                decoration: const InputDecoration(
                    hintText: 'Mood, genre or theme… e.g. dreamy shoegaze')),
            Wrap(spacing: 6, children: [
              for (final word in [
                'Bright pop',
                'Melancholy indie',
                'Dreamy ambient',
                'Dark metal',
                'Warm jazz',
                'Blues'
              ])
                ActionChip(
                    label: Text(word),
                    onPressed: _locked
                        ? null
                        : () => setState(() => _prompt.text = word))
            ]),
            DropdownButtonFormField<String>(
                initialValue: _voicing,
                decoration:
                    const InputDecoration(labelText: 'Suggested shapes'),
                items: const [
                  DropdownMenuItem(
                      value: 'comfortable',
                      child: Text('Comfortable transitions')),
                  DropdownMenuItem(
                      value: 'open', child: Text('Prefer open shapes')),
                  DropdownMenuItem(
                      value: 'movable', child: Text('Prefer movable shapes')),
                ],
                onChanged:
                    _locked ? null : (v) => setState(() => _voicing = v!)),
            const Text(
                'A preference, not a restriction. Shapes remain editable.',
                style: TextStyle(fontSize: 12, color: LostChrome.ice)),
            const SizedBox(height: 8),
            Wrap(spacing: 8, children: [
              FilledButton.tonal(
                  onPressed: _locked ? null : () => _suggest(false),
                  child: const Text('Offline ideas')),
              OutlinedButton.icon(
                  onPressed: _locked || !_ai ? null : () => _suggest(true),
                  icon: const Icon(Icons.auto_awesome),
                  label: const Text('Ask AI')),
            ]),
            Text(
                _ai
                    ? 'Ask AI sends your prompt, tonal center, shape preference and relevant local musical recipes to OpenAI. No recordings are sent.'
                    : 'AI unavailable: configure PEDAL_OPENAI_API_KEY and restart the catalog.',
                style: const TextStyle(fontSize: 12, color: LostChrome.ice)),
            for (final p in _suggestions)
              ListTile(
                  contentPadding: EdgeInsets.zero,
                  title: Text(p['name']),
                  subtitle: Text(
                      '${p['source']} • ${(p['steps'] as List).map((s) => _chords[s['chord']]!.label).join(' → ')}'),
                  trailing: const Icon(Icons.add_circle_outline),
                  onTap: _locked ? null : () => _apply(p)),
          ])),
      _panel(
          'SAVED PROGRESSIONS',
          Column(children: [
            if (_saved.isEmpty)
              const Text(
                  'Saved progressions stay on this device and work offline.'),
            for (final p in _saved)
              ListTile(
                  contentPadding: EdgeInsets.zero,
                  title: Text(p['name']),
                  subtitle: Text(
                      '${p['key']} • ${p['bpm']} BPM • ${(p['steps'] as List).length} chords'),
                  onTap: _locked ? null : () => _apply(p),
                  trailing: IconButton(
                      tooltip: 'Delete saved progression',
                      icon: const Icon(Icons.delete_outline),
                      onPressed: _locked
                          ? null
                          : () async {
                              if (!await _confirm('Delete ${p['name']}?',
                                  'This removes it from your local library.')) {
                                return;
                              }
                              if (!mounted) return;
                              setState(() => _busy = true);
                              final updated = _saved
                                  .where((item) => item['id'] != p['id'])
                                  .toList();
                              try {
                                await _store.save(updated);
                                if (mounted) {
                                  setState(() {
                                    _saved = updated;
                                    if (_id == p['id']) {
                                      _id = null;
                                      _dirty = true;
                                    }
                                  });
                                }
                              } catch (e) {
                                if (mounted) {
                                  setState(
                                      () => _error = 'Could not delete: $e');
                                }
                              } finally {
                                if (mounted) setState(() => _busy = false);
                              }
                            })),
          ])),
    ]);
    return ExcludeSemantics(
        excluding: _variationDialogOpen,
        child: SingleChildScrollView(
            padding: const EdgeInsets.all(12),
            child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  if (_busy) const LinearProgressIndicator(),
                  if (_error != null)
                    Padding(
                        padding: const EdgeInsets.only(bottom: 12),
                        child: Text(_error!,
                            style: const TextStyle(color: Color(0xffffbccb)))),
                  LayoutBuilder(
                      builder: (context, size) => size.maxWidth >= 760
                          ? Row(
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: [
                                  SizedBox(width: 320, child: browser),
                                  const SizedBox(width: 12),
                                  Expanded(child: builder)
                                ])
                          : Column(children: [browser, builder])),
                ])));
  }
}

/// Standard chord chart: vertical strings, horizontal frets, real finger labels.
class ChordShapePainter extends CustomPainter {
  ChordShapePainter(this.shape);
  final Map<String, dynamic> shape;
  @override
  void paint(Canvas canvas, Size size) {
    final frets = List<int>.from(shape['frets']),
        fingers = List<int>.from(shape['fingers']);
    final positive = frets.where((f) => f > 0).toList();
    final start = positive.isEmpty || positive.reduce(math.max) <= 4
        ? 1
        : positive.reduce(math.min);
    const left = 38.0, top = 44.0, gap = 38.0;
    final dx = (size.width - 65) / 5;
    void label(String text, double x, double y,
        {Color color = LostChrome.ice, double font = 15}) {
      final painter = TextPainter(
          text: TextSpan(
              text: text,
              style: TextStyle(
                  color: color,
                  fontFamily: 'Arial',
                  fontSize: font,
                  fontWeight: FontWeight.w800)),
          textDirection: TextDirection.ltr)
        ..layout();
      painter.paint(
          canvas, Offset(x - painter.width / 2, y - painter.height / 2));
    }

    for (var f = 0; f <= 5; f++) {
      canvas.drawLine(
          Offset(left, top + f * gap),
          Offset(left + 5 * dx, top + f * gap),
          Paint()
            ..color = LostChrome.ice
            ..strokeWidth = f == 0 && start == 1 ? 5 : 1.2);
      if (f < 5) label('${start + f}', 16, top + (f + .5) * gap, font: 12);
    }
    for (int finger = 1; finger <= 4; finger++) {
      final positions = <int>[];
      for (int s = 0; s < 6; s++) {
        if (fingers[s] == finger && frets[s] > 0) positions.add(s);
      }
      if (positions.length > 1 &&
          positions.every((s) => frets[s] == frets[positions.first])) {
        final y = top + (frets[positions.first] - start + .5) * gap;
        canvas.drawLine(
            Offset(left + positions.first * dx, y),
            Offset(left + positions.last * dx, y),
            Paint()
              ..color = LostChrome.lime.withValues(alpha: .6)
              ..strokeWidth = 18
              ..strokeCap = StrokeCap.round);
      }
    }
    for (int s = 0; s < 6; s++) {
      final x = left + s * dx;
      canvas.drawLine(
          Offset(x, top),
          Offset(x, top + 5 * gap),
          Paint()
            ..color = LostChrome.ice
            ..strokeWidth = 1.6 - s * .15);
      label(['E', 'A', 'D', 'G', 'B', 'e'][s], x, top + 5 * gap + 18, font: 12);
      if (frets[s] <= 0) {
        label(frets[s] < 0 ? '×' : '○', x, top - 20, font: 21);
        continue;
      }
      final y = top + (frets[s] - start + .5) * gap;
      canvas.drawCircle(Offset(x, y), 13, Paint()..color = LostChrome.lime);
      label('${fingers[s]}', x, y, color: LostChrome.midnight);
    }
  }

  @override
  bool shouldRepaint(covariant ChordShapePainter oldDelegate) =>
      oldDelegate.shape != shape;
}
