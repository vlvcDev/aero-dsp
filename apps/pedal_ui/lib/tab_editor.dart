part of 'main.dart';

const _tabTunings = {
  'standard': [40, 45, 50, 55, 59, 64],
  'drop_d': [38, 45, 50, 55, 59, 64],
  'half_down': [39, 44, 49, 54, 58, 63],
  'whole_down': [38, 43, 48, 53, 57, 62],
  'drop_c': [36, 43, 48, 53, 57, 62],
};

class GuitarTabEditor extends StatefulWidget {
  const GuitarTabEditor(
      {super.key,
      required this.initialJob,
      required this.catalog,
      required this.clip,
      this.client,
      this.onBack});
  final Map<String, dynamic> initialJob;
  final CatalogClient catalog;
  final ControlClient? client;
  final TimelineClip clip;
  final VoidCallback? onBack;
  @override
  State<GuitarTabEditor> createState() => _GuitarTabEditorState();
}

class _GuitarTabEditorState extends State<GuitarTabEditor> {
  late Map<String, dynamic> _job;
  late List<Map<String, dynamic>> _notes;
  final _undo = <List<Map<String, dynamic>>>[];
  final _redo = <List<Map<String, dynamic>>>[];
  final _scroll = ScrollController();
  Timer? _poll;
  int _playGeneration = 0;
  bool _dirty = false,
      _saving = false,
      _playing = false,
      _transportBusy = false;
  bool _ownsPlayback = false, _loop = false, _wet = false, _allowClose = false;
  bool _confirming = false;
  double _position = 0, _pixelsPerSecond = 100;
  late RangeValues _range;
  int? _selected;
  String? _error;
  double get _duration => (_job['duration_seconds'] as num).toDouble();
  int get _bpm => (_job['bpm'] as num).toInt();
  List<int> get _tuning =>
      _tabTunings[_job['tuning']] ?? _tabTunings['standard']!;
  List<Map<String, dynamic>> _copy(dynamic notes) =>
      (notes as List).map((n) => Map<String, dynamic>.from(n as Map)).toList();

  @override
  void initState() {
    super.initState();
    _job = Map.of(widget.initialJob);
    _notes = _copy(_job['notes']);
    _range = RangeValues(0, _duration);
  }

  @override
  void dispose() {
    _poll?.cancel();
    _scroll.dispose();
    if (_ownsPlayback && !_transportBusy) {
      widget.client?.timelineCommand(
          'timeline_stop', {}).catchError((Object _) => <String, dynamic>{});
    }
    super.dispose();
  }

  void _change(List<Map<String, dynamic>> notes) {
    _undo.add(_copy(_notes));
    if (_undo.length > 50) _undo.removeAt(0);
    setState(() {
      _notes = notes
        ..sort((a, b) =>
            (a['start_seconds'] as num).compareTo(b['start_seconds'] as num));
      _redo.clear();
      _dirty = true;
      _selected = null;
    });
  }

  Future<bool> _discardAllowed() async {
    if (!_dirty) return true;
    return await showDialog<bool>(
            context: context,
            builder: (context) => AlertDialog(
                  title: const Text('Discard unsaved tab edits?'),
                  content: const Text(
                      'Save corrections before leaving if you want to keep them.'),
                  actions: [
                    TextButton(
                        onPressed: () => Navigator.pop(context, false),
                        child: const Text('Keep editing')),
                    TextButton(
                        onPressed: () => Navigator.pop(context, true),
                        child: const Text('Discard'))
                  ],
                )) ??
        false;
  }

  Future<void> _save() async {
    setState(() {
      _saving = true;
      _error = null;
    });
    try {
      final job = await widget.catalog.transcriptionCommand('save', {
        'id': _job['id'],
        'revision': _job['revision'] ?? 0,
        'notes': _notes,
      });
      if (mounted) {
        setState(() {
          _job = job;
          _notes = _copy(job['notes']);
          _dirty = false;
        });
      }
    } catch (error) {
      if (mounted) setState(() => _error = 'Save failed: $error');
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  Future<void> _stop() async {
    _playGeneration++;
    _poll?.cancel();
    if (_ownsPlayback) {
      await widget.client!.timelineCommand('timeline_stop', {});
      _ownsPlayback = false;
    }
    if (mounted) setState(() => _playing = false);
  }

  Future<void> _play() async {
    if (_transportBusy || widget.client == null) return;
    setState(() {
      _transportBusy = true;
      _error = null;
    });
    try {
      await _stop();
      final status =
          await widget.client!.timelineCommand('timeline_status', {});
      if (status['recording'] == true) {
        throw Exception('Stop recording before auditioning tabs.');
      }
      if (!mounted) return;
      _ownsPlayback =
          true; // Also recover starts accepted after a control timeout.
      final position = _position >= _range.start && _position < _range.end
          ? _position
          : _range.start;
      await widget.client!.timelineCommand('timeline_start', {
        'name': 'Tab review',
        'record': false,
        'count_in_bars': 0,
        'click': false,
        'bpm': _bpm,
        'loop': _loop,
        'start_ms': ((position - _range.start) * 1000).round(),
        'tracks': [
          {
            'id': 'tab-review',
            'volume': 1.0,
            'pan': 0.0,
            'muted': false,
            'solo': false,
            'clips': [
              {
                'path': _wet ? widget.clip.takePath : widget.clip.cleanTakePath,
                'start_ms': 0,
                'source_in_ms': widget.clip.sourceStartMilliseconds +
                    (_range.start * 1000).round(),
                'length_ms': ((_range.end - _range.start) * 1000).round(),
              }
            ]
          }
        ],
      });
      if (!mounted) {
        await _stop();
        return;
      }
      setState(() => _playing = true);
      final generation = _playGeneration;
      bool polling = false;
      _poll = Timer.periodic(const Duration(milliseconds: 40), (_) async {
        if (polling) return;
        polling = true;
        try {
          final state =
              await widget.client!.timelineCommand('timeline_status', {});
          if (!mounted || !_ownsPlayback || generation != _playGeneration) {
            return;
          }
          final seconds = (state['position_frames'] as num).toDouble() /
              (state['sample_rate'] as num).toDouble();
          setState(() => _position =
              (_range.start + seconds).clamp(_range.start, _range.end));
          if (_scroll.hasClients) {
            final x = _position * _pixelsPerSecond;
            if (x < _scroll.offset ||
                x > _scroll.offset + _scroll.position.viewportDimension - 80) {
              _scroll
                  .jumpTo((x - 80).clamp(0, _scroll.position.maxScrollExtent));
            }
          }
          if (state['interrupted'] == true) {
            throw Exception('Audio interrupted; playback stopped.');
          }
          if (state['playing'] != true) await _stop();
        } catch (error) {
          _poll?.cancel();
          if (mounted) {
            setState(() {
              _playing = false;
              _error = '$error';
            });
          }
        } finally {
          polling = false;
        }
      });
    } catch (error) {
      if (mounted) setState(() => _error = 'Playback: $error');
    } finally {
      if (mounted) setState(() => _transportBusy = false);
    }
  }

  Future<void> _edit([int? index]) async {
    final note = index == null
        ? <String, dynamic>{
            'start_seconds': math.min(_position, math.max(0, _duration - .1)),
            'end_seconds': math.min(_position + .25, _duration),
            'midi': _tuning[5],
            'string': 1,
            'fret': 0,
            'strength': .8,
            'bends': <int>[],
          }
        : Map<String, dynamic>.of(_notes[index]);
    final result = await showAeroControlBay<Map<String, dynamic>>(context,
        title: index == null ? 'Add Note' : 'Correct Note',
        icon: Icons.music_note,
        child: _TabNoteForm(note: note, tuning: _tuning, duration: _duration));
    if (result == null || !mounted) return;
    final notes = _copy(_notes);
    if (index == null) {
      notes.add(result);
    } else {
      notes[index] = result;
    }
    _change(notes);
  }

  Future<void> _setRange() async {
    final start =
        TextEditingController(text: (_range.start * 1000).round().toString());
    final end =
        TextEditingController(text: (_range.end * 1000).round().toString());
    String? error;
    final result = await showDialog<RangeValues>(
        context: context,
        builder: (context) => StatefulBuilder(
            builder: (context, update) => AlertDialog(
                  title: const Text('Playback section'),
                  content: SingleChildScrollView(
                      child: Column(mainAxisSize: MainAxisSize.min, children: [
                    TextField(
                        controller: start,
                        keyboardType: TextInputType.number,
                        decoration:
                            const InputDecoration(labelText: 'Start (ms)')),
                    TextField(
                        controller: end,
                        keyboardType: TextInputType.number,
                        decoration:
                            const InputDecoration(labelText: 'End (ms)')),
                    if (error != null) Text(error!),
                  ])),
                  actions: [
                    TextButton(
                        onPressed: () => Navigator.pop(context),
                        child: const Text('Cancel')),
                    FilledButton(
                        onPressed: () {
                          final a = int.tryParse(start.text),
                              b = int.tryParse(end.text);
                          if (a == null ||
                              b == null ||
                              a < 0 ||
                              b <= a ||
                              b > (_duration * 1000).round()) {
                            update(() => error =
                                'Choose a valid range inside this clip.');
                            return;
                          }
                          Navigator.pop(
                              context, RangeValues(a / 1000, b / 1000));
                        },
                        child: const Text('Apply'))
                  ],
                )));
    // Dialog route animations may still hold the fields for this frame.
    Future<void>.delayed(const Duration(seconds: 1), () {
      start.dispose();
      end.dispose();
    });
    if (result != null && mounted) {
      setState(() {
        _range = result;
        _position = result.start;
      });
    }
  }

  @override
  Widget build(BuildContext context) => PopScope(
        canPop: !_dirty && !_saving && !_transportBusy || _allowClose,
        onPopInvokedWithResult: (didPop, result) async {
          if (didPop || _confirming || _saving || _transportBusy) return;
          _confirming = true;
          final leave = await _discardAllowed();
          _confirming = false;
          if (leave && mounted) {
            setState(() => _allowClose = true);
            WidgetsBinding.instance.addPostFrameCallback((_) {
              if (mounted) Navigator.pop(context);
            });
          }
        },
        child: ListView(padding: const EdgeInsets.all(12), children: [
          Text('TAB WORKBENCH · $_bpm BPM',
              style: const TextStyle(
                  color: LostChrome.ice,
                  fontSize: 22,
                  fontWeight: FontWeight.w900)),
          Text(
              '${_dirty ? 'UNSAVED EDITS' : 'SAVED LOCALLY'} · ${_notes.length} notes · ${_job['tuning']}',
              style: const TextStyle(color: LostChrome.lime)),
          const Text(
              'Tap a fret to select it, then Edit. Tap empty space to seek. '
              'Beat lines follow the project timeline. Playback auditions the recording, not synthesized edits.'),
          Wrap(spacing: 6, children: [
            FilledButton.icon(
                onPressed: _saving || !_dirty ? null : _save,
                icon: const Icon(Icons.save),
                label: Text(_saving ? 'Saving…' : 'Save edits')),
            TextButton(
                onPressed: _saving
                    ? null
                    : () {
                        final step = 60 / _bpm / 4;
                        final offset =
                            widget.clip.timelineStartMilliseconds / 1000;
                        final notes = _copy(_notes);
                        for (final note in notes) {
                          final start =
                              (note['start_seconds'] as num).toDouble();
                          final length =
                              (note['end_seconds'] as num).toDouble() - start;
                          final snapped = (((start + offset) / step).round() *
                                      step -
                                  offset)
                              .clamp(0.0, math.max(0.0, _duration - length));
                          note['start_seconds'] = snapped;
                          note['end_seconds'] =
                              math.min(_duration, snapped + length);
                        }
                        _change(notes);
                      },
                child: const Text('Snap starts to 1/16')),
            IconButton(
                tooltip: 'Undo',
                onPressed: _saving || _undo.isEmpty
                    ? null
                    : () => setState(() {
                          _redo.add(_copy(_notes));
                          _notes = _undo.removeLast();
                          _dirty = true;
                          _selected = null;
                        }),
                icon: const Icon(Icons.undo)),
            IconButton(
                tooltip: 'Redo',
                onPressed: _saving || _redo.isEmpty
                    ? null
                    : () => setState(() {
                          _undo.add(_copy(_notes));
                          _notes = _redo.removeLast();
                          _dirty = true;
                          _selected = null;
                        }),
                icon: const Icon(Icons.redo)),
            TextButton(
                onPressed: _saving
                    ? null
                    : () =>
                        _change(_copy(_job['original_notes'] ?? _job['notes'])),
                child: const Text('Restore original')),
            if (widget.onBack != null)
              TextButton(
                  onPressed: _saving || _transportBusy
                      ? null
                      : () async {
                          if (await _discardAllowed()) {
                            try {
                              await _stop();
                              if (mounted) widget.onBack!();
                            } catch (error) {
                              if (mounted) setState(() => _error = '$error');
                            }
                          }
                        },
                  child: const Text('Choose another clip')),
          ]),
          if (_error != null)
            Text(_error!, style: const TextStyle(color: Color(0xffffaabd))),
          Wrap(
              spacing: 6,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                FilledButton.icon(
                    onPressed: widget.client == null || _transportBusy
                        ? null
                        : () async {
                            if (_playing || _ownsPlayback) {
                              try {
                                await _stop();
                              } catch (e) {
                                if (mounted) setState(() => _error = '$e');
                              }
                            } else {
                              await _play();
                            }
                          },
                    icon: Icon(_playing ? Icons.stop : Icons.play_arrow),
                    label: Text(_playing ? 'Stop' : 'Play')),
                FilterChip(
                    label: const Text('Loop section'),
                    selected: _loop,
                    onSelected: _playing || _transportBusy
                        ? null
                        : (v) => setState(() => _loop = v)),
                FilterChip(
                    label: Text(_wet ? 'Recorded rig' : 'Clean DI'),
                    selected: _wet,
                    onSelected: _playing || _transportBusy
                        ? null
                        : (v) => setState(() => _wet = v)),
                TextButton(
                    onPressed: _playing || _transportBusy ? null : _setRange,
                    child: Text(
                        '${_range.start.toStringAsFixed(3)}–${_range.end.toStringAsFixed(3)} s')),
                Text('${_position.toStringAsFixed(3)} s'),
              ]),
          Row(children: [
            const Text('Zoom'),
            Expanded(
                child: Slider(
                    value: _pixelsPerSecond,
                    min: 40,
                    max: 300,
                    onChanged: (v) => setState(() => _pixelsPerSecond = v)))
          ]),
          Container(
              height: 270,
              clipBehavior: Clip.hardEdge,
              decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(14),
                  border: Border.all(color: LostChrome.ice),
                  gradient: const LinearGradient(
                      colors: [Color(0xff12456c), LostChrome.navy])),
              child: ScrollConfiguration(
                behavior: const MaterialScrollBehavior().copyWith(dragDevices: {
                  ui.PointerDeviceKind.mouse,
                  ui.PointerDeviceKind.touch,
                  ui.PointerDeviceKind.trackpad
                }),
                child: Scrollbar(
                    controller: _scroll,
                    thumbVisibility: true,
                    child: SingleChildScrollView(
                      controller: _scroll,
                      scrollDirection: Axis.horizontal,
                      child: GestureDetector(
                          onTapUp: (tap) {
                            final x = tap.localPosition.dx,
                                y = tap.localPosition.dy;
                            int? nearest;
                            double distance = 26;
                            for (var i = 0; i < _notes.length; i++) {
                              final n = _notes[i];
                              if (n['string'] == null) continue;
                              final dx = x -
                                  (48 +
                                      (n['start_seconds'] as num) *
                                          _pixelsPerSecond);
                              final dy =
                                  y - (48 + ((n['string'] as int) - 1) * 34);
                              final d = math.sqrt(dx * dx + dy * dy);
                              if (d < distance) {
                                nearest = i;
                                distance = d;
                              }
                            }
                            setState(() {
                              _selected = nearest;
                              if (!_playing) {
                                _position = ((x - 48) / _pixelsPerSecond)
                                    .clamp(0, _duration);
                              }
                            });
                          },
                          child: CustomPaint(
                              size: Size(
                                  math.max(
                                      380, 96 + _duration * _pixelsPerSecond),
                                  260),
                              painter: _TabScorePainter(
                                  notes: _notes,
                                  tuning: _tuning,
                                  bpm: _bpm,
                                  pixelsPerSecond: _pixelsPerSecond,
                                  timelineOffset:
                                      widget.clip.timelineStartMilliseconds /
                                          1000,
                                  position: _position,
                                  selected: _selected))),
                    )),
              )),
          Wrap(spacing: 6, children: [
            OutlinedButton.icon(
                onPressed: _saving ? null : () => _edit(),
                icon: const Icon(Icons.add),
                label: const Text('Add note')),
            OutlinedButton.icon(
                onPressed: _saving || _selected == null
                    ? null
                    : () => _edit(_selected),
                icon: const Icon(Icons.edit),
                label: const Text('Edit note')),
            OutlinedButton.icon(
                onPressed: _saving || _selected == null
                    ? null
                    : () {
                        final notes = _copy(_notes)..removeAt(_selected!);
                        _change(notes);
                      },
                icon: const Icon(Icons.delete_outline),
                label: const Text('Delete note')),
          ]),
          if (_notes.any((n) => n['string'] == null))
            ExpansionTile(
                title: Text(
                    '${_notes.where((n) => n['string'] == null).length} notes need fingering review'),
                children: [
                  for (var i = 0; i < _notes.length; i++)
                    if (_notes[i]['string'] == null)
                      ListTile(
                          title: Text(
                              'MIDI ${_notes[i]['midi']} at ${(_notes[i]['start_seconds'] as num).toStringAsFixed(3)} s'),
                          trailing: const Icon(Icons.edit),
                          onTap: _saving ? null : () => _edit(i))
                ]),
          TextButton.icon(
              onPressed: _dirty
                  ? null
                  : () => Clipboard.setData(
                      ClipboardData(text: _job['tab'] as String)),
              icon: const Icon(Icons.copy),
              label: const Text('Copy saved tabs')),
          SelectableText('Tab: ${_job['tab_path']}\nMIDI: ${_job['midi_path']}',
              style: const TextStyle(color: LostChrome.ice, fontSize: 12)),
          const Text(
              'Save updates the local tab and MIDI exports. Original model notes are retained. '
              'Pitch-motion curves remain in note data; corrected MIDI exports contain note pitches and timing only.'),
        ]),
      );
}

class _TabNoteForm extends StatefulWidget {
  const _TabNoteForm(
      {required this.note, required this.tuning, required this.duration});
  final Map<String, dynamic> note;
  final List<int> tuning;
  final double duration;
  @override
  State<_TabNoteForm> createState() => _TabNoteFormState();
}

class _TabNoteFormState extends State<_TabNoteForm> {
  late final TextEditingController start, end, pitch, fret;
  int? _string;
  String? _error;
  @override
  void initState() {
    super.initState();
    _string = widget.note['string'] as int?;
    start = TextEditingController(
        text:
            ((widget.note['start_seconds'] as num) * 1000).round().toString());
    end = TextEditingController(
        text: ((widget.note['end_seconds'] as num) * 1000).round().toString());
    pitch = TextEditingController(text: '${widget.note['midi']}');
    fret = TextEditingController(text: '${widget.note['fret'] ?? 0}');
  }

  @override
  void dispose() {
    start.dispose();
    end.dispose();
    pitch.dispose();
    fret.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final midi = int.tryParse(pitch.text) ?? -1;
    final choices = [
      for (var s = 1; s <= 6; s++)
        if (midi - widget.tuning[6 - s] >= 0 &&
            midi - widget.tuning[6 - s] <= 24)
          s
    ];
    return ListView(padding: const EdgeInsets.all(16), children: [
      TextField(
          controller: pitch,
          keyboardType: TextInputType.number,
          decoration: const InputDecoration(
              labelText: 'MIDI pitch (0–127)',
              helperText: 'Changing pitch clears the string assignment.'),
          onChanged: (_) => setState(() => _string = null)),
      const SizedBox(height: 12),
      DropdownButtonFormField<int>(
          key: ValueKey('$midi/$_string'),
          initialValue: _string ?? 0,
          isExpanded: true,
          decoration: const InputDecoration(
              labelText: 'Fingering · string 1 is highest'),
          items: [
            const DropdownMenuItem(
                value: 0, child: Text('Unassigned — review later')),
            for (final s in choices)
              DropdownMenuItem(
                  value: s,
                  child:
                      Text('String $s · fret ${midi - widget.tuning[6 - s]}'))
          ],
          onChanged: (value) => setState(() {
                _string = value == 0 ? null : value;
                if (_string != null) {
                  fret.text = '${midi - widget.tuning[6 - _string!]}';
                }
              })),
      if (_string != null)
        TextField(
          controller: fret,
          keyboardType: TextInputType.number,
          decoration: const InputDecoration(
              labelText: 'Fret (0–24)',
              helperText: 'Changing fret updates the note pitch.'),
          onChanged: (text) {
            final value = int.tryParse(text);
            if (value != null && value >= 0 && value <= 24) {
              setState(
                  () => pitch.text = '${widget.tuning[6 - _string!] + value}');
            }
          },
        ),
      const SizedBox(height: 12),
      TextField(
          controller: start,
          keyboardType: TextInputType.number,
          decoration: const InputDecoration(labelText: 'Start (milliseconds)')),
      TextField(
          controller: end,
          keyboardType: TextInputType.number,
          decoration: const InputDecoration(labelText: 'End (milliseconds)')),
      if (_error != null)
        Text(_error!, style: const TextStyle(color: Color(0xffffaabd))),
      const SizedBox(height: 16),
      FilledButton(
          onPressed: () {
            final a = int.tryParse(start.text), b = int.tryParse(end.text);
            if (a == null ||
                b == null ||
                a < 0 ||
                b <= a ||
                b > (widget.duration * 1000).round() ||
                midi < 0 ||
                midi > 127) {
              setState(() =>
                  _error = 'Enter a valid pitch and times within the clip.');
              return;
            }
            if (_string != null &&
                (int.tryParse(fret.text) !=
                    midi - widget.tuning[6 - _string!])) {
              setState(() => _error = 'Enter a fret between 0 and 24.');
              return;
            }
            Navigator.pop(context, {
              ...widget.note,
              'midi': midi,
              'string': _string,
              'fret':
                  _string == null ? null : midi - widget.tuning[6 - _string!],
              'start_seconds': a / 1000,
              'end_seconds': b / 1000,
              if (midi != widget.note['midi']) 'bends': <int>[],
            });
          },
          child: const Text('Apply note')),
    ]);
  }
}

class _TabScorePainter extends CustomPainter {
  const _TabScorePainter(
      {required this.notes,
      required this.tuning,
      required this.bpm,
      required this.pixelsPerSecond,
      required this.timelineOffset,
      required this.position,
      required this.selected});
  final List<Map<String, dynamic>> notes;
  final List<int> tuning;
  final int bpm;
  final double pixelsPerSecond, position, timelineOffset;
  final int? selected;
  void label(
      Canvas canvas, String value, Offset point, Color color, double size) {
    final text = TextPainter(
        text: TextSpan(
            text: value,
            style: TextStyle(
                color: color, fontSize: size, fontWeight: FontWeight.bold)),
        textDirection: TextDirection.ltr)
      ..layout();
    text.paint(canvas, point);
  }

  @override
  void paint(Canvas canvas, Size size) {
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
    for (var i = 0; i < 6; i++) {
      final y = 48.0 + i * 34;
      canvas.drawLine(Offset(40, y), Offset(size.width, y),
          Paint()..color = LostChrome.ice.withValues(alpha: .6));
      label(canvas, names[tuning[5 - i] % 12], Offset(8, y - 10),
          LostChrome.ice, 16);
    }
    final step = 60 / bpm * pixelsPerSecond;
    final offset = timelineOffset * pixelsPerSecond;
    for (var beat = (timelineOffset * bpm / 60).ceil();
        48 + beat * step - offset < size.width;
        beat++) {
      final x = 48 + beat * step - offset;
      canvas.drawLine(Offset(x, 30), Offset(x, 238),
          Paint()..color = LostChrome.ice.withValues(alpha: .25));
      label(canvas, '${beat + 1}', Offset(x, 6), LostChrome.ice, 11);
    }
    for (var i = 0; i < notes.length; i++) {
      final n = notes[i];
      if (n['string'] == null) continue;
      final x = 48 + (n['start_seconds'] as num) * pixelsPerSecond;
      final end = 48 + (n['end_seconds'] as num) * pixelsPerSecond;
      final clip = canvas.getLocalClipBounds();
      if (end < clip.left - 30 || x > clip.right + 30) continue;
      final y = 48.0 + ((n['string'] as int) - 1) * 34;
      final active = position >= (n['start_seconds'] as num) &&
          position < (n['end_seconds'] as num);
      final color = i == selected
          ? LostChrome.lime
          : active
              ? LostChrome.electric
              : LostChrome.ice;
      canvas.drawLine(
          Offset(x, y + 8),
          Offset(end, y + 8),
          Paint()
            ..color = color.withValues(alpha: .65)
            ..strokeWidth = 4);
      canvas.drawRRect(
          RRect.fromRectAndRadius(
              Rect.fromCenter(center: Offset(x, y), width: 30, height: 27),
              const Radius.circular(6)),
          Paint()..color = i == selected ? LostChrome.ocean : LostChrome.navy);
      label(canvas, '${n['fret']}', Offset(x - 8, y - 11), color, 18);
    }
    final cursor = 48 + position * pixelsPerSecond;
    canvas.drawLine(
        Offset(cursor, 28),
        Offset(cursor, 240),
        Paint()
          ..color = LostChrome.lime
          ..strokeWidth = 2);
  }

  @override
  bool shouldRepaint(covariant _TabScorePainter oldDelegate) => true;
}
