part of 'main.dart';

/// Editable offline document. Layout is deliberately not part of the DSP schema.
class PatchDocument {
  String assetDirectory = Directory.current.absolute.path;
  String previewPath = '';
  final positions = <int, Offset>{};
  Map<String, dynamic> data = {
    'version': 1,
    'sample_rate': 48000,
    'graph': {
      'version': 1,
      'nodes': [
        {
          'id': 1,
          'inputs': <int>[],
          'processor': {'type': 'input'}
        },
        {
          'id': 2,
          'inputs': [1],
          'processor': {'type': 'output'}
        },
      ],
    },
    'processors': <String, dynamic>{},
  };
  List<dynamic> get nodes => data['graph']['nodes'] as List<dynamic>;
  Map<String, dynamic> get processors =>
      data['processors'] as Map<String, dynamic>;
  String encode() => const JsonEncoder.withIndent('  ').convert(data);

  void importJson(String text) {
    final candidate = PatchDocument()
      ..data = jsonDecode(text) as Map<String, dynamic>;
    final error = candidate.validate();
    if (error != null) throw FormatException(error);
    data = candidate.data;
    positions.clear();
  }

  /// Check a single new cable even while the rest of a draft is incomplete.
  String? connectionError(int source, int destination) {
    final byId = {for (final n in nodes) n['id'] as int: n as Map};
    if (!byId.containsKey(source) || !byId.containsKey(destination)) {
      return 'That node no longer exists.';
    }
    if (byId[source]!['processor']['type'] == 'output' ||
        byId[destination]!['processor']['type'] == 'input') {
      return 'Connect an output socket to an input socket.';
    }
    final inputs = byId[destination]!['inputs'] as List;
    if (inputs.contains(source)) return 'These nodes are already connected.';
    if (byId[destination]!['processor']['type'] == 'mixer' &&
        inputs.length >= 8) {
      return 'A mixer supports at most eight inputs.';
    }
    final seen = <int>{};
    bool reaches(int id) {
      if (id == destination) return true;
      if (!seen.add(id)) return false;
      return (byId[id]?['inputs'] as List? ?? []).any((v) => reaches(v as int));
    }

    return reaches(source) ? 'Feedback connections are not supported.' : null;
  }

  String? validate({int depth = 0, List<int>? budget, int? expectedRate}) {
    budget ??= [0, 0];
    try {
      if (depth > 4) {
        return 'Custom nodes support at most four levels of nesting.';
      }
      void keys(Map value, Set<String> allowed) {
        if (value.keys.any((key) => !allowed.contains(key))) {
          throw const FormatException('Unsupported document fields.');
        }
      }

      keys(data, {'version', 'sample_rate', 'graph', 'processors'});
      keys(data['graph'] as Map, {'version', 'nodes'});
      final rate = data['sample_rate'];
      if (data['version'] != 1 ||
          data['graph']['version'] != 1 ||
          rate is! int ||
          rate < 8000 ||
          rate > 192000) {
        return 'Use version 1 and a sample rate between 8000 and 192000 Hz.';
      }
      if (expectedRate != null && rate != expectedRate) {
        return 'Custom nodes must use the same sample rate.';
      }
      budget[0] += nodes.length;
      if (budget[0] > 64) return 'A patch supports up to 64 nodes in total.';
      final byId = <int, Map>{};
      var delays = 0;
      for (final raw in nodes) {
        final node = raw as Map;
        keys(node, {'id', 'inputs', 'processor'});
        final id = node['id'] as int;
        if (id < 0 || id > 0xffffffff || byId.containsKey(id)) {
          return 'Node IDs must be unique unsigned integers.';
        }
        byId[id] = node;
        final inputs = List<int>.from(node['inputs'] as List);
        if (inputs.toSet().length != inputs.length) {
          return 'Remove duplicate connections on node $id.';
        }
        final p = node['processor'] as Map;
        final type = p['type'];
        if (!['input', 'output', 'gain', 'mixer', 'delay', 'external']
            .contains(type)) {
          return 'Unsupported processor on node $id.';
        }
        keys(p,
            {'type', if (type == 'gain') 'db', if (type == 'delay') 'samples'});
        final required = type == 'input' ? 0 : 1;
        if (type == 'mixer'
            ? inputs.length < 2 || inputs.length > 8
            : inputs.length != required) {
          return 'Node $id needs ${type == 'mixer' ? '2–8 inputs' : '$required input(s)'}.';
        }
        if (type == 'gain') {
          final db = p['db'] as num;
          if (!db.isFinite || db < -80 || db > 24) {
            return 'Gain must be −80 to +24 dB.';
          }
        }
        if (type == 'delay') {
          final samples = p['samples'] as int;
          if (samples < 0) return 'Delay cannot be negative.';
          delays += samples;
        }
        if (type == 'external') {
          final native = processors['$id'] as Map?;
          if (native == null) {
            return 'Node $id needs native processor settings.';
          }
          final kind = native['type'];
          if (!['nam', 'cabinet', 'reverb', 'custom', 'fuzz'].contains(kind)) {
            return 'Unsupported native processor.';
          }
          keys(native, {
            'type',
            if (kind == 'nam' || kind == 'cabinet') 'path',
            if (kind == 'custom') ...[
              'name',
              'library_id',
              'revision',
              'asset_directory',
              'document'
            ],
            if (kind == 'nam') 'input',
            if (kind == 'cabinet') 'duration_seconds',
            if (kind == 'reverb') 'decay_seconds',
            if (kind == 'fuzz') ...[
              'drive_db',
              'bias',
              'tone',
              'mix',
              'output_db'
            ]
          });
          if (kind == 'fuzz') {
            for (final entry in {
              'drive_db': [0.0, 36.0],
              'bias': [-1.0, 1.0],
              'tone': [0.0, 1.0],
              'mix': [0.0, 1.0],
              'output_db': [-36.0, 6.0],
            }.entries) {
              final value = native[entry.key];
              if (value is! num ||
                  !value.isFinite ||
                  value < entry.value[0] ||
                  value > entry.value[1]) {
                return 'Fuzz ${entry.key} is outside its supported range.';
              }
            }
          }
          if (kind == 'custom') {
            if (native['name'] is! String ||
                (native['name'] as String).trim().isEmpty ||
                native['library_id'] is! String ||
                !RegExp(r'^[a-zA-Z0-9_-]{1,80}$')
                    .hasMatch(native['library_id'] as String) ||
                native['revision'] is! int ||
                native['revision'] < 1 ||
                native['asset_directory'] is! String ||
                !(native['asset_directory'] as String).startsWith('/')) {
              return 'Invalid custom node metadata.';
            }
            final child = PatchDocument()
              ..data = Map<String, dynamic>.from(native['document'] as Map);
            final error = child.validate(
                depth: depth + 1, budget: budget, expectedRate: rate);
            if (error != null) return error;
          }
          if (kind == 'reverb' || kind == 'cabinet') {
            final v =
                native[kind == 'reverb' ? 'decay_seconds' : 'duration_seconds']
                    as num;
            if (!v.isFinite ||
                v < (kind == 'reverb' ? .3 : .02) ||
                v > (kind == 'reverb' ? 12 : 1)) {
              return 'Native processor duration is outside its supported range.';
            }
          }
          if (kind == 'nam' || kind == 'cabinet') {
            final path = native['path'] as String;
            if (path.trim().isEmpty ||
                path.startsWith('/') ||
                path.split('/').contains('..')) {
              return 'Use an asset path relative to the saved rig folder.';
            }
          }
          if (kind == 'nam' &&
              !['left', 'right', 'average'].contains(native['input'])) {
            return 'Select a NAM input channel.';
          }
        }
      }
      budget[1] += delays;
      if (budget[1] > 192000) {
        return 'Combined delays exceed the 192000-frame budget.';
      }
      for (final key in processors.keys) {
        final id = int.tryParse(key);
        if (id == null ||
            '$id' != key ||
            byId[id]?['processor']['type'] != 'external') {
          return 'Native settings must belong to an external node.';
        }
      }
      for (final type in ['input', 'output']) {
        if (byId.values.where((n) => n['processor']['type'] == type).length !=
            1) {
          return 'A patch needs exactly one $type node.';
        }
      }
      final visiting = <int>{}, visited = <int>{};
      String? walk(int id) {
        if (visiting.contains(id)) {
          return 'Feedback connections are not supported.';
        }
        if (visited.contains(id)) return null;
        visiting.add(id);
        for (final source in byId[id]!['inputs'] as List) {
          if (!byId.containsKey(source)) {
            return 'A connection points to a missing node.';
          }
          if (byId[source]!['processor']['type'] == 'output') {
            return 'Output cannot feed another node.';
          }
          final error = walk(source as int);
          if (error != null) return error;
        }
        visiting.remove(id);
        visited.add(id);
        return null;
      }

      final output = byId.entries
          .firstWhere((e) => e.value['processor']['type'] == 'output')
          .key;
      final error = walk(output);
      if (error != null) return error;
      if (visited.length != nodes.length) {
        return 'Connect every node to the output, or remove unused nodes.';
      }
      return null;
    } catch (_) {
      return 'Invalid or incomplete rig document.';
    }
  }
}

class PatchEditor extends StatefulWidget {
  const PatchEditor(
      {super.key, required this.document, this.client, this.library});
  final PatchDocument document;
  final ControlClient? client;
  final CustomNodeLibrary? library;
  @override
  State<PatchEditor> createState() => _PatchEditorState();
}

class _PatchEditorState extends State<PatchEditor> {
  late final CustomNodeLibrary library =
      widget.library ?? CustomNodeLibrary.defaultLibrary();
  bool _canvasMode = true;
  int? _selected;
  final _undo = <String>[];
  final _redo = <String>[];
  PatchDocument get doc => widget.document;
  String snapshot() => jsonEncode({
        'document': doc.data,
        'positions': {
          for (final entry in doc.positions.entries)
            '${entry.key}': [entry.value.dx, entry.value.dy]
        },
      });
  void connect(int source, int destination) {
    final error = doc.connectionError(source, destination);
    if (error != null) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text(error)));
      return;
    }
    edit(() {
      final node = doc.nodes.firstWhere((n) => n['id'] == destination);
      final inputs = node['inputs'] as List;
      if (node['processor']['type'] != 'mixer') inputs.clear();
      inputs.add(source);
    });
  }

  void edit(VoidCallback action) {
    setState(() {
      _undo.add(snapshot());
      if (_undo.length > 50) _undo.removeAt(0);
      _redo.clear();
      action();
    });
  }

  void history(List<String> from, List<String> to) => setState(() {
        to.add(snapshot());
        final saved = jsonDecode(from.removeLast()) as Map<String, dynamic>;
        doc.data = saved['document'] as Map<String, dynamic>;
        doc.positions.clear();
        for (final entry
            in (saved['positions'] as Map<String, dynamic>).entries) {
          doc.positions[int.parse(entry.key)] = Offset(
              (entry.value[0] as num).toDouble(),
              (entry.value[1] as num).toDouble());
        }
      });
  String label(Map n) =>
      '${n['id']} · ${n['processor']['type'] == 'external' ? (doc.processors['${n['id']}']['type'] == 'custom' ? doc.processors['${n['id']}']['name'] : doc.processors['${n['id']}']['type']) : n['processor']['type']}'
          .toUpperCase();

  void add(String type) => edit(() {
        final id =
            doc.nodes.fold<int>(0, (v, n) => math.max(v, n['id'] as int)) + 1;
        final external = ['nam', 'cabinet', 'reverb', 'fuzz'].contains(type);
        doc.nodes.add({
          'id': id,
          'inputs': <int>[],
          'processor': {
            'type': external ? 'external' : type,
            if (type == 'gain') 'db': 0.0,
            if (type == 'delay') 'samples': 4800,
          }
        });
        if (external) {
          doc.processors['$id'] = {
            'type': type,
            if (type == 'nam' || type == 'cabinet') 'path': '',
            if (type == 'fuzz') ...{
              'drive_db': 18.0,
              'bias': 0.0,
              'tone': .5,
              'mix': 1.0,
              'output_db': -12.0,
            },
            if (type == 'nam') 'input': 'average',
            if (type == 'cabinet') 'duration_seconds': .2,
            if (type == 'reverb') 'decay_seconds': 2.5,
          };
        }
      });

  void message(String value) => ScaffoldMessenger.of(context)
      .showSnackBar(SnackBar(content: Text(value)));

  Future<void> saveCustom() async {
    final name = TextEditingController();
    String? existing;
    String? error;
    List<CustomNodeRevision> latest;
    try {
      latest = library.latest();
    } catch (e) {
      message('$e');
      return;
    }
    await showDialog<void>(
        context: context,
        builder: (context) => StatefulBuilder(
            builder: (context, refresh) => AlertDialog(
                  title: const Text('Save as Custom Node'),
                  content: Column(mainAxisSize: MainAxisSize.min, children: [
                    TextField(
                        controller: name,
                        decoration: InputDecoration(
                            labelText: 'Name', errorText: error)),
                    DropdownButton<String>(
                        value: existing ?? '',
                        isExpanded: true,
                        items: [
                          const DropdownMenuItem(
                              value: '', child: Text('Create new node')),
                          for (final item in latest)
                            DropdownMenuItem(
                                value: item.id,
                                child: Text('New revision of ${item.name}')),
                        ],
                        onChanged: (v) => refresh(() {
                              existing = v == '' ? null : v;
                              if (existing != null) {
                                name.text = latest
                                    .firstWhere((r) => r.id == existing)
                                    .name;
                              }
                            })),
                  ]),
                  actions: [
                    TextButton(
                        onPressed: () => Navigator.pop(context),
                        child: const Text('Cancel')),
                    FilledButton(
                        onPressed: () {
                          try {
                            library.save(doc, name.text, id: existing);
                            Navigator.pop(context);
                            message('Custom node saved.');
                          } catch (e) {
                            refresh(() => error = '$e');
                          }
                        },
                        child: const Text('Save'))
                  ],
                )));
    WidgetsBinding.instance.addPostFrameCallback((_) => name.dispose());
  }

  Future<void> browseCustom() async {
    List<CustomNodeRevision> latest;
    try {
      latest = library.latest();
    } catch (e) {
      message('$e');
      return;
    }
    final chosen = await showDialog<CustomNodeRevision>(
        context: context,
        builder: (context) => AlertDialog(
              title: const Text('My Nodes'),
              content: SizedBox(
                  width: 400,
                  child: latest.isEmpty
                      ? const Text('No saved nodes yet.')
                      : SingleChildScrollView(
                          child:
                              Column(mainAxisSize: MainAxisSize.min, children: [
                          for (final r in latest)
                            ListTile(
                                title: Text(r.name),
                                subtitle: Text('Revision ${r.revision}'),
                                trailing: const Icon(Icons.add),
                                onTap: () => Navigator.pop(context, r)),
                        ]))),
              actions: [
                TextButton(
                    onPressed: () => Navigator.pop(context),
                    child: const Text('Close'))
              ],
            ));
    if (chosen == null || !mounted) return;
    if (chosen.document['sample_rate'] != doc.data['sample_rate']) {
      message('Custom node has a different sample rate.');
      return;
    }
    edit(() {
      final id =
          doc.nodes.fold<int>(0, (v, n) => math.max(v, n['id'] as int)) + 1;
      doc.nodes.add({
        'id': id,
        'inputs': <int>[],
        'processor': {'type': 'external'}
      });
      doc.processors['$id'] = chosen.toProcessor();
      _selected = id;
    });
  }

  Future<void> editEmbedded(int id) async {
    final native = doc.processors['$id'] as Map;
    final child = PatchDocument()
      ..data =
          jsonDecode(jsonEncode(native['document'])) as Map<String, dynamic>
      ..assetDirectory = native['asset_directory'] as String;
    final committed = await showDialog<bool>(
        context: context,
        builder: (context) => Dialog(
            child: SizedBox(
                width: 1100,
                height: 750,
                child: Column(children: [
                  Row(children: [
                    Expanded(child: Text('  Edit ${native['name']}')),
                    TextButton(
                        onPressed: () => Navigator.pop(context, false),
                        child: const Text('Cancel')),
                    FilledButton(
                        onPressed: () {
                          final error = child.validate();
                          if (error != null) {
                            ScaffoldMessenger.of(context)
                                .showSnackBar(SnackBar(content: Text(error)));
                            return;
                          }
                          Navigator.pop(context, true);
                        },
                        child: const Text('Apply changes'))
                  ]),
                  Expanded(
                      child: PatchEditor(document: child, library: library)),
                ]))));
    if (committed != true || !mounted) return;
    final previous = native['document'];
    native['document'] = child.data;
    final error = doc.validate();
    native['document'] = previous;
    if (error != null) {
      message(error);
      return;
    }
    edit(() {
      native['document'] = jsonDecode(jsonEncode(child.data));
      native['asset_directory'] = child.assetDirectory;
    });
  }

  void updateCustom(int id) {
    try {
      final current = doc.processors['$id'] as Map;
      final matches =
          library.latest().where((r) => r.id == current['library_id']);
      if (matches.isEmpty) {
        message('This node is no longer in the library.');
        return;
      }
      final replacement = matches.first.toProcessor();
      doc.processors['$id'] = replacement;
      final error = doc.validate();
      doc.processors['$id'] = current;
      if (error != null) {
        message(error);
        return;
      }
      edit(() => doc.processors['$id'] = replacement);
      message('Updated from library.');
    } catch (e) {
      message('$e');
    }
  }

  Future<void> jsonDialog(bool importing) async {
    final controller =
        TextEditingController(text: importing ? '' : doc.encode());
    String? error;
    await showDialog<void>(
        context: context,
        builder: (context) => StatefulBuilder(
            builder: (context, refresh) => AlertDialog(
                  title:
                      Text(importing ? 'Import rig JSON' : 'Export rig JSON'),
                  content: SizedBox(
                      width: 640,
                      child: Column(mainAxisSize: MainAxisSize.min, children: [
                        const Text(
                            'Save JSON beside its NAM / IR assets. Asset paths are relative to that folder. The native renderer checks files and model compatibility.'),
                        const SizedBox(height: 12),
                        TextField(
                            controller: controller,
                            readOnly: !importing,
                            minLines: 5,
                            maxLines: 10,
                            decoration: InputDecoration(
                                border: const OutlineInputBorder(),
                                errorText: error)),
                      ])),
                  actions: [
                    TextButton(
                        onPressed: () => Navigator.pop(context),
                        child: const Text('Close')),
                    FilledButton(
                        onPressed: () async {
                          if (importing) {
                            try {
                              final candidate = PatchDocument()
                                ..importJson(controller.text);
                              edit(() {
                                doc.data = candidate.data;
                                doc.positions.clear();
                                _selected = null;
                              });
                              Navigator.pop(context);
                            } catch (e) {
                              refresh(() => error = e.toString());
                            }
                          } else {
                            await Clipboard.setData(
                                ClipboardData(text: controller.text));
                            if (context.mounted) Navigator.pop(context);
                          }
                        },
                        child: Text(importing ? 'Import' : 'Copy JSON'))
                  ],
                )));
    // Dialog exit animations can still reference its controller for one frame.
    WidgetsBinding.instance.addPostFrameCallback((_) => controller.dispose());
  }

  Widget number(Map target, String key, String title, double min, double max,
      {bool integer = false, bool percent = false}) {
    final value = (target[key] as num).toDouble();
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      Text(
          '$title: ${percent ? '${(value * 100).round()}%' : integer ? value.round() : value.toStringAsFixed(1)}'),
      Slider(
          value: value.clamp(min, max),
          min: min,
          max: max,
          onChangeStart: (_) {
            _undo.add(snapshot());
            if (_undo.length > 50) _undo.removeAt(0);
            _redo.clear();
          },
          onChanged: (v) =>
              setState(() => target[key] = integer ? v.round() : v)),
    ]);
  }

  Widget nodeCard(Map node) {
    final id = node['id'];
    final p = node['processor'] as Map;
    final type = p['type'];
    final native = doc.processors['$id'] as Map?;
    final inputs = node['inputs'] as List;
    return Container(
        width: 280,
        padding: const EdgeInsets.all(16),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(20),
          border: Border.all(color: LostChrome.ice),
          gradient: const LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [LostChrome.ocean, LostChrome.navy]),
        ),
        child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            mainAxisSize: MainAxisSize.min,
            children: [
              Row(children: [
                Expanded(
                    child: Text(label(node),
                        style: const TextStyle(
                            color: LostChrome.pearl,
                            fontWeight: FontWeight.bold))),
                if (type != 'input' && type != 'output')
                  IconButton(
                      tooltip: 'Remove node',
                      icon: const Icon(Icons.close),
                      onPressed: () => edit(() {
                            doc.nodes.remove(node);
                            doc.processors.remove('$id');
                            for (final n in doc.nodes) {
                              (n['inputs'] as List).remove(id);
                            }
                          })),
              ]),
              if (type != 'input') ...[
                const Text('INPUT CONNECTIONS',
                    style: TextStyle(color: LostChrome.ice)),
                Wrap(spacing: 4, children: [
                  for (final source in doc.nodes)
                    if (source['id'] != id &&
                        source['processor']['type'] != 'output')
                      FilterChip(
                          label: Text(label(source as Map)),
                          selected: inputs.contains(source['id']),
                          onSelected: (selected) {
                            if (selected) {
                              connect(source['id'] as int, id as int);
                            } else {
                              edit(() => inputs.remove(source['id']));
                            }
                          }),
                ]),
              ],
              if (type == 'gain') number(p, 'db', 'Gain · dB', -80, 24),
              if (type == 'delay')
                number(p, 'samples', 'Delay · samples', 0, 192000,
                    integer: true),
              if (native?['type'] == 'reverb')
                number(native!, 'decay_seconds', 'Decay · seconds', .3, 12),
              if (native?['type'] == 'fuzz') ...[
                number(native!, 'drive_db', 'Drive · dB', 0, 36),
                number(native, 'bias', 'Bias · asymmetry', -1, 1),
                number(native, 'tone', 'Tone · darker to brighter', 0, 1,
                    percent: true),
                number(native, 'mix', 'Mix · wet', 0, 1, percent: true),
                number(native, 'output_db', 'Output · dB', -36, 6),
              ],
              if (native?['type'] == 'custom') ...[
                Text('Revision ${native!['revision']}'),
                TextButton(
                    onPressed: () => editEmbedded(id as int),
                    child: const Text('Inspect / edit custom node')),
                TextButton(
                    onPressed: () => updateCustom(id as int),
                    child: const Text('Update from library')),
              ],
              if (native != null &&
                  (native['type'] == 'nam' || native['type'] == 'cabinet')) ...[
                TextFormField(
                    key: ValueKey('$id-${native['path']}'),
                    initialValue: native['path'] as String,
                    decoration: const InputDecoration(
                        labelText: 'Relative asset path',
                        helperText: 'Enter path, then press Enter'),
                    onFieldSubmitted: (v) =>
                        edit(() => native['path'] = v.trim())),
                if (native['type'] == 'nam')
                  DropdownButton<String>(
                      value: native['input'] as String,
                      items: [
                        for (final v in ['left', 'right', 'average'])
                          DropdownMenuItem(value: v, child: Text('Input: $v'))
                      ],
                      onChanged: (v) => edit(() => native['input'] = v)),
                if (native['type'] == 'cabinet')
                  number(native, 'duration_seconds', 'IR length · seconds', .02,
                      1),
              ],
              if (type == 'input')
                const Text('Stereo source • native sample rate'),
              if (type == 'mixer')
                const Text('Sums 2–8 inputs. Use gain nodes for headroom.'),
            ]));
  }

  @override
  Widget build(BuildContext context) {
    final error = doc.validate();
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      Padding(
          padding: const EdgeInsets.all(12),
          child: Text(
              widget.client == null
                  ? 'OFFLINE PATCH LAB — export JSON to keep your draft.'
                  : 'Apply to guitar to hear this draft. Edits take effect when reapplied. Export JSON to keep your work after closing the app.',
              style: const TextStyle(color: LostChrome.ice))),
      Wrap(
          spacing: 8,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            if (widget.client != null)
              TextButton.icon(
                icon: const Icon(Icons.play_circle_outline),
                label: const Text('Guitar / Preview'),
                onPressed: () => showAeroControlBay<void>(context,
                    title: 'PATCH AUDIO',
                    icon: Icons.graphic_eq,
                    child: SingleChildScrollView(
                        child: PatchAudioControls(
                            document: doc, client: widget.client!))),
              ),
            PopupMenuButton<String>(
                tooltip: 'Add node',
                onSelected: add,
                itemBuilder: (_) => [
                      for (final type in [
                        'gain',
                        'mixer',
                        'delay',
                        'nam',
                        'cabinet',
                        'reverb',
                        'fuzz'
                      ])
                        PopupMenuItem(
                            value: type, child: Text(type.toUpperCase()))
                    ],
                child: const Padding(
                    padding: EdgeInsets.all(12), child: Text('+ ADD NODE'))),
            TextButton(
                onPressed: error == null ? saveCustom : null,
                child: const Text('Save as Custom Node')),
            TextButton(onPressed: browseCustom, child: const Text('My Nodes')),
            IconButton(
                tooltip: 'Undo',
                onPressed: _undo.isEmpty ? null : () => history(_undo, _redo),
                icon: const Icon(Icons.undo)),
            IconButton(
                tooltip: 'Redo',
                onPressed: _redo.isEmpty ? null : () => history(_redo, _undo),
                icon: const Icon(Icons.redo)),
            TextButton(
                onPressed: () => jsonDialog(true),
                child: const Text('Import JSON')),
            TextButton(
                onPressed: error == null ? () => jsonDialog(false) : null,
                child: const Text('Export JSON')),
            Text('${doc.data['sample_rate']} Hz'),
            IconButton(
                tooltip:
                    _canvasMode ? 'Show connection cards' : 'Show node canvas',
                onPressed: () => setState(() => _canvasMode = !_canvasMode),
                icon: Icon(_canvasMode
                    ? Icons.view_agenda_outlined
                    : Icons.account_tree_outlined)),
          ]),
      Padding(
          padding: const EdgeInsets.all(12),
          child: Text(error ?? 'Routing valid • ready for offline rendering',
              style: TextStyle(
                  color: error == null ? LostChrome.lime : LostChrome.pearl))),
      Expanded(
          child: _canvasMode
              ? LayoutBuilder(builder: (context, constraints) {
                  final selected =
                      doc.nodes.where((n) => n['id'] == _selected).firstOrNull;
                  final canvas = PatchCanvas(
                    document: doc,
                    selected: _selected,
                    label: label,
                    onSelect: (id) => setState(() => _selected = id),
                    onConnect: connect,
                    onMoveStart: () => edit(() {}),
                  );
                  if (selected == null) return canvas;
                  final inspector = SingleChildScrollView(
                    child: Column(mainAxisSize: MainAxisSize.min, children: [
                      TextButton.icon(
                          onPressed: () => setState(() => _selected = null),
                          icon: const Icon(Icons.close),
                          label: const Text('Close controls')),
                      nodeCard(selected as Map),
                    ]),
                  );
                  if (constraints.maxWidth >= 800) {
                    return Row(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          Expanded(child: canvas),
                          SizedBox(width: 300, child: inspector)
                        ]);
                  }
                  // On short/touch displays, controls replace the canvas until closed.
                  return Center(child: inspector);
                })
              : SingleChildScrollView(
                  padding: const EdgeInsets.all(12),
                  child: Wrap(spacing: 16, runSpacing: 16, children: [
                    for (final node in doc.nodes) nodeCard(node as Map)
                  ]))),
    ]);
  }
}
