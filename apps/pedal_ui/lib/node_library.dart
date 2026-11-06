part of 'main.dart';

/// A revision is immutable. Instances embed its contents and never resolve a
/// moving library reference while rendering or reopening a project.
class CustomNodeRevision {
  CustomNodeRevision(
      {required this.id,
      required this.revision,
      required this.name,
      required String documentJson,
      required this.assetDirectory})
      : _documentJson = documentJson;
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
      if (!byId.containsKey(item.id) || byId[item.id]!.revision < item.revision) {
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

  CustomNodeRevision save(PatchDocument document, String name, {String? id}) {
    final error = document.validate();
    if (error != null) throw FormatException(error);
    if (name.trim().isEmpty) {
      throw const FormatException('Give your node a name');
    }
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
            if (relative.startsWith('/') || relative.split('/').contains('..')) {
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
