part of 'main.dart';

/// A revision is immutable. Instances embed its contents and never resolve a
/// moving library reference while rendering or reopening a project.
class CustomNodeRevision {
  CustomNodeRevision(
      {required this.id,
      required this.revision,
      required this.name,
      required String documentJson,
      required this.assetDirectory,
      List<NodeMacro> macros = const []})
      : macros = List.unmodifiable(macros),
        _documentJson = documentJson;
  final List<NodeMacro> macros;
  final String id;
  final int revision;
  final String name;
  final String assetDirectory;
  final String _documentJson;
  Map<String, dynamic> toProcessor() =>
      {...toJson()..remove('version'), 'type': 'custom'};
  Map<String, dynamic> get document =>
      jsonDecode(_documentJson) as Map<String, dynamic>;
  Map<String, dynamic> toJson() => {
        'version': 1,
        'library_id': id,
        'revision': revision,
        'name': name,
        'asset_directory': assetDirectory,
        if (macros.isNotEmpty) 'macros': macros.map((m) => m.toJson()).toList(),
        'document': document
      };

  factory CustomNodeRevision.fromJson(Map<String, dynamic> json) {
    if (json['version'] != 1 ||
        json['revision'] is! int ||
        (json['revision'] as int) < 1 ||
        json['name'] is! String ||
        (json['name'] as String).trim().isEmpty ||
        json['asset_directory'] is! String ||
        json['library_id'] is! String ||
        !RegExp(r'^[a-zA-Z0-9_-]{1,80}$')
            .hasMatch(json['library_id'] as String)) {
      throw const FormatException('Invalid custom node revision');
    }
    final document = PatchDocument()..importJson(jsonEncode(json['document']));
    return CustomNodeRevision(
        id: json['library_id'] as String,
        revision: json['revision'] as int,
        name: json['name'] as String,
        macros: readNodeMacros(json, document),
        documentJson: document.encode(),
        assetDirectory: json['asset_directory'] as String);
  }

  /// Explicit update: return a fresh copy. Neither this instance nor other rigs
  /// are mutated; the editor can record the replacement in its undo history.
  CustomNodeRevision updatedFrom(CustomNodeRevision replacement) {
    if (replacement.id != id) {
      throw const FormatException('Choose the same library node');
    }
    if (replacement.document['sample_rate'] != document['sample_rate']) {
      throw const FormatException(
          'Library revision has a different sample rate');
    }
    return CustomNodeRevision.fromJson(replacement.toJson());
  }
}

class CustomNodeLibrary {
  CustomNodeLibrary(this.directory);
  factory CustomNodeLibrary.defaultLibrary() {
    final base = Platform.environment['XDG_DATA_HOME'] ??
        '${Platform.environment['HOME']}/.local/share';
    return CustomNodeLibrary(Directory('$base/aero-dsp/custom-nodes'));
  }
  List<CustomNodeRevision> latest() {
    final byId = <String, CustomNodeRevision>{};
    for (final item in revisions()) {
      if (!byId.containsKey(item.id) ||
          byId[item.id]!.revision < item.revision) {
        byId[item.id] = item;
      }
    }
    return byId.values.toList()..sort((a, b) => a.name.compareTo(b.name));
  }

  final Directory directory;
  List<CustomNodeRevision> revisions() {
    if (!directory.existsSync()) return [];
    final result = <CustomNodeRevision>[];
    for (final file
        in directory.listSync(followLinks: false).whereType<File>()) {
      if (!file.path.endsWith('.json')) continue;
      if (file.lengthSync() > 2 * 1024 * 1024) {
        throw const FormatException('Custom node exceeds 2 MiB');
      }
      result.add(CustomNodeRevision.fromJson(
          jsonDecode(file.readAsStringSync()) as Map<String, dynamic>));
    }
    return result
      ..sort((a, b) => a.id == b.id
          ? a.revision.compareTo(b.revision)
          : a.name.compareTo(b.name));
  }

  CustomNodeRevision save(PatchDocument document, String name,
      {String? id, List<NodeMacro> macros = const []}) {
    final error = document.validate();
    if (error != null) throw FormatException(error);
    if (name.trim().isEmpty) {
      throw const FormatException('Give your node a name');
    }
    readNodeMacros(
        {'macros': macros.map((m) => m.toJson()).toList()}, document);
    final nodeId = id ?? 'node-${DateTime.now().microsecondsSinceEpoch}';
    if (!RegExp(r'^[a-zA-Z0-9_-]{1,80}$').hasMatch(nodeId)) {
      throw const FormatException('Invalid library identifier');
    }
    final revision = revisions()
            .where((r) => r.id == nodeId)
            .fold<int>(0, (value, r) => math.max(value, r.revision)) +
        1;
    directory.createSync(recursive: true);
    final target = File('${directory.path}/$nodeId-$revision.json');
    final temporary = File('${target.path}.pending');
    if (target.existsSync() || temporary.existsSync()) {
      throw const FileSystemException('Revision already exists');
    }
    final assets =
        Directory('${directory.absolute.path}/$nodeId-$revision-assets');
    if (assets.existsSync()) {
      throw const FileSystemException('Revision assets already exist');
    }
    assets.createSync();
    try {
      final frozen = jsonDecode(document.encode()) as Map<String, dynamic>;
      var index = 0;
      void copyAssets(Map<String, dynamic> graph, String root) {
        for (final raw in (graph['processors'] as Map).values) {
          final processor = raw as Map;
          if (processor['type'] == 'custom') {
            copyAssets(processor['document'] as Map<String, dynamic>,
                processor['asset_directory'] as String);
            processor['asset_directory'] = assets.path;
          } else if (processor['type'] == 'nam' ||
              processor['type'] == 'cabinet') {
            final relative = processor['path'] as String;
            if (relative.startsWith('/') ||
                relative.split('/').contains('..')) {
              throw const FormatException('Asset path leaves its folder');
            }
            final base = Directory(root).resolveSymbolicLinksSync();
            final source = File('$base/$relative').resolveSymbolicLinksSync();
            if (!source.startsWith('${base == '/' ? '' : base}/')) {
              throw const FormatException('Asset symlink leaves its folder');
            }
            final name =
                'asset-${index++}.${processor['type'] == 'nam' ? 'nam' : 'wav'}';
            File(source).copySync('${assets.path}/$name');
            processor['path'] = name;
          }
        }
      }

      copyAssets(frozen, document.assetDirectory);
      final saved = CustomNodeRevision(
          id: nodeId,
          revision: revision,
          name: name.trim(),
          macros: macros,
          documentJson: jsonEncode(frozen),
          assetDirectory: assets.path);
      temporary.writeAsStringSync(jsonEncode(saved.toJson()), flush: true);
      temporary.renameSync(target.path);
      return saved;
    } catch (_) {
      if (temporary.existsSync()) temporary.deleteSync();
      assets.deleteSync(recursive: true);
      rethrow;
    }
  }
}

/// An exposed control points at the actual parameter in an instance's snapshot.
/// Saving and rendering always use the same value that the inspector displays.
class NodeMacro {
  const NodeMacro(this.nodeId, this.parameter, this.label);
  final int nodeId;
  final String parameter;
  final String label;
  String get target => '$nodeId/$parameter';
  Map<String, dynamic> toJson() =>
      {'node_id': nodeId, 'parameter': parameter, 'label': label.trim()};
}

typedef _Control = ({
  int id,
  String type,
  String parameter,
  String label,
  Map settings,
  double min,
  double max,
  bool percent
});
Iterable<_Control> _macroTargets(PatchDocument doc) sync* {
  for (final node in doc.nodes) {
    final id = node['id'] as int;
    final p = node['processor'] as Map;
    final target =
        (p['type'] == 'external' ? doc.processors['$id'] : p) as Map?;
    if (target == null) continue;
    final type = target['type'] as String;
    final List<(String, String, double, double, bool)> specs = switch (type) {
      'gain' => [('db', 'Gain (dB)', -80, 24, false)],
      'edge_amp' => [('drive_db','Preamp drive (dB)',0,30,false),('tone','Tone',0,1,true),('touch','Touch compression',0,1,true),('master_db','Master (dB)',-36,6,false)],
      'reverb' => [('decay_seconds', 'Decay (s)', .3, 12, false)],
      'cabinet' => [('duration_seconds', 'Impulse length (s)', .02, 1, false)],
      'fuzz' => [
          ('drive_db', 'Drive (dB)', 0, 36, false),
          ('bias', 'Bias', -1, 1, false),
          ('tone', 'Tone', 0, 1, true),
          ('mix', 'Mix', 0, 1, true),
          ('output_db', 'Output (dB)', -36, 6, false)
        ],
      'overdrive' => [
          ('drive_db', 'Drive (dB)', 0, 24, false),
          ('tone', 'Tone', 0, 1, true),
          ('mix', 'Mix', 0, 1, true),
          ('output_db', 'Output (dB)', -24, 6, false)
        ],
      'tremolo' || 'chorus' || 'flanger' => [
          (
            'rate_hz',
            'Rate (Hz)',
            type == 'tremolo' ? .1 : .05,
            type == 'chorus'
                ? 8
                : type == 'flanger'
                    ? 5
                    : 18,
            false
          ),
          ('depth', 'Depth', 0, 1, true),
          if (type != 'tremolo') ('mix', 'Mix', 0, 1, true),
          if (type == 'flanger') ('feedback', 'Feedback', -.85, .85, true),
        ],
      'octaver' => [('mix', 'Octave mix', 0, 1, true)],
      _ => [],
    };
    for (final s in specs) {
      if (target[s.$1] is num) {
        yield (
          id: id,
          type: type,
          parameter: s.$1,
          label: s.$2,
          settings: target,
          min: s.$3,
          max: s.$4,
          percent: s.$5
        );
      }
    }
  }
}

List<NodeMacro> compatibleMacros(
    PatchDocument document, Iterable<NodeMacro> macros) {
  final targets = {
    for (final c in _macroTargets(document)) '${c.id}/${c.parameter}'
  };
  return macros.where((m) => targets.contains(m.target)).toList();
}

List<NodeMacro> readNodeMacros(Map wrapper, PatchDocument document) {
  if (!wrapper.containsKey('macros')) return [];
  final raw = wrapper['macros'];
  if (raw is! List || raw.length > 4) {
    throw const FormatException('Choose up to four exposed controls.');
  }
  final macros = <NodeMacro>[];
  final used = <String>{};
  for (final item in raw) {
    if (item is! Map ||
        item.length != 3 ||
        item['node_id'] is! int ||
        item['parameter'] is! String ||
        item['label'] is! String) {
      throw const FormatException('Invalid exposed control.');
    }
    final m = NodeMacro(
        item['node_id'], item['parameter'], (item['label'] as String).trim());
    if (m.label.isEmpty || m.label.length > 32 || !used.add(m.target)) {
      throw const FormatException(
          'Give each exposed control a name and a distinct target.');
    }
    macros.add(m);
  }
  if (compatibleMacros(document, macros).length != macros.length) {
    throw const FormatException(
        'An exposed control has no matching parameter.');
  }
  return macros;
}

typedef MacroNumberBuilder = Widget Function(
    Map target, String key, String title, double min, double max,
    {bool integer, bool percent});

Iterable<Widget> macroSliders(Map native, MacroNumberBuilder number) sync* {
  final child = PatchDocument()
    ..data = native['document'] as Map<String, dynamic>;
  final controls = {
    for (final c in _macroTargets(child)) '${c.id}/${c.parameter}': c
  };
  for (final m in readNodeMacros(native, child)) {
    final c = controls[m.target]!;
    yield number(c.settings, c.parameter, m.label, c.min, c.max,
        integer: false, percent: c.percent);
  }
}

class MacroPicker extends StatelessWidget {
  const MacroPicker(
      {super.key,
      required this.document,
      required this.macros,
      required this.onChanged});
  final PatchDocument document;
  final List<NodeMacro> macros;
  final ValueChanged<List<NodeMacro>> onChanged;
  @override
  Widget build(BuildContext context) {
    final targets = _macroTargets(document).toList();
    if (targets.isEmpty) return const SizedBox.shrink();
    final available = targets.where((c) =>
        !macros.any((m) => m.nodeId == c.id && m.parameter == c.parameter));
    return Column(mainAxisSize: MainAxisSize.min, children: [
      for (final m in macros)
        Row(key: ValueKey(m.target), children: [
          Expanded(
              child: TextFormField(
                  initialValue: m.label,
                  maxLength: 32,
                  decoration: InputDecoration(
                      labelText: 'Control name · ${m.nodeId} / ${m.parameter}',
                      counterText: ''),
                  onChanged: (label) => onChanged([
                        for (final v in macros)
                          v.target == m.target
                              ? NodeMacro(v.nodeId, v.parameter, label)
                              : v
                      ]))),
          IconButton(
              tooltip: 'Remove exposed control',
              icon: const Icon(Icons.close),
              onPressed: () => onChanged(
                  macros.where((v) => v.target != m.target).toList())),
        ]),
      if (macros.length < 4 && available.isNotEmpty)
        DropdownButton<_Control>(
            isExpanded: true,
            hint: const Text('Expose a control'),
            items: [
              for (final c in available)
                DropdownMenuItem(
                    value: c, child: Text('${c.id} · ${c.type} · ${c.label}'))
            ],
            onChanged: (c) {
              if (c != null) {
                onChanged([...macros, NodeMacro(c.id, c.parameter, c.label)]);
              }
            }),
    ]);
  }
}
