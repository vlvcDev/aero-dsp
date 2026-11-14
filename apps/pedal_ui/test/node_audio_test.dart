import 'dart:async';
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

class PatchTestClient extends ControlClient {
  bool active = false;
  Map<String, Object?>? applied;
  final updates = <Map<String, Object?>>[];
  String? revision;
  @override
  Future<Map<String, dynamic>> patchRequest(String type,
      [Map<String, Object?> payload = const {}]) async {
    if (type == 'patch_status') {
      return {
        'available': true,
        'active': active,
        'revision': revision,
        'faulted': false,
        'switching': false,
        'sample_rate': 48000
      };
    }
    if (type == 'apply_patch') {
      applied = payload;
      active = true;
      revision = 'r1';
      return {'queued': true, 'revision': revision};
    }
    if (type == 'update_patch') {
      updates.add(payload);
      return {'updated': true, 'revision': revision};
    }
    if (type == 'deactivate_patch') active = false;
    if (type == 'render_patch') throw Exception('Preview WAV is missing');
    return {'queued': true};
  }
}

class DelayedClient extends PatchTestClient {
  final update = Completer<Map<String, dynamic>>();
  @override
  Future<Map<String, dynamic>> patchRequest(String type,
      [Map<String, Object?> payload = const {}]) {
    if (type == 'update_patch') return update.future;
    return super.patchRequest(type, payload);
  }
}

class DelayedApplyClient extends PatchTestClient {
  final activation = Completer<Map<String, dynamic>>();
  @override
  Future<Map<String, dynamic>> patchRequest(String type,
      [Map<String, Object?> payload = const {}]) {
    if (type == 'apply_patch') return activation.future;
    return super.patchRequest(type, payload);
  }
}

void main() {
  testWidgets('closing the audio panel during Apply retains live ownership',
      (tester) async {
    final client = DelayedApplyClient();
    final doc = PatchDocument();
    final live = PatchLiveController(doc, client);
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: PatchAudioControls(
                document: doc, client: client, live: live))));
    await tester.pump();
    await tester.tap(find.text('Apply to guitar'));
    await tester.pump();
    await tester.pumpWidget(const SizedBox());
    client.active = true;
    client.revision = 'late';
    client.activation.complete({'queued': true, 'revision': 'late'});
    await tester.pump();
    expect(live.revision, 'late');
    expect(live.label, 'Tuning live');
    expect(tester.takeException(), isNull);
    live.dispose();
  });
  testWidgets('nested macro and undo send live updates through the editor',
      (tester) async {
    tester.view.physicalSize = const Size(1500, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final folder = Directory.systemTemp.createTempSync('live-macro-');
    addTearDown(() => folder.deleteSync(recursive: true));
    final library = CustomNodeLibrary(folder);
    final child = PatchDocument();
    child.nodes.add({
      'id': 3,
      'inputs': [1],
      'processor': {'type': 'gain', 'db': -6.0}
    });
    child.nodes[1]['inputs'] = [3];
    library
        .save(child, 'Boost', macros: [const NodeMacro(3, 'db', 'Loudness')]);
    final doc = PatchDocument();
    doc.nodes.add({
      'id': 3,
      'inputs': [1],
      'processor': {'type': 'external'}
    });
    doc.nodes[1]['inputs'] = [3];
    doc.processors['3'] = library.latest().single.toProcessor();
    final client = PatchTestClient();
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body:
                PatchEditor(document: doc, client: client, library: library))));
    await tester.tap(find.text('Guitar / Preview'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Apply to guitar'));
    await tester.pumpAndSettle();
    Navigator.of(tester.element(find.text('Apply to guitar'))).pop();
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('patch-handle-3')));
    await tester.pump();
    final row = find
        .ancestor(
            of: find.textContaining('Loudness:'), matching: find.byType(Column))
        .first;
    final slider =
        find.descendant(of: row, matching: find.byType(Slider)).first;
    await tester.drag(slider, const Offset(50, 0));
    await tester.pump(const Duration(milliseconds: 50));
    await tester.pump();
    expect(client.updates, isNotEmpty);
    final changed = client.updates.last['document'] as Map;
    expect(
        changed['processors']['3']['document']['graph']['nodes'][2]['processor']
            ['db'],
        greaterThan(-6));
    await tester.tap(find.byTooltip('Undo'));
    await tester.pump(const Duration(milliseconds: 50));
    await tester.pump();
    final undone = client.updates.last['document'] as Map;
    expect(
        undone['processors']['3']['document']['graph']['nodes'][2]['processor']
            ['db'],
        -6);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('old in-flight rejection cannot revoke a newly applied revision',
      (tester) async {
    final client = DelayedClient();
    final doc = PatchDocument();
    doc.nodes.insert(1, {
      'id': 3,
      'inputs': [1],
      'processor': {'type': 'gain', 'db': 0.0}
    });
    doc.nodes.last['inputs'] = [3];
    final live = PatchLiveController(doc, client);
    client.active = true;
    client.revision = 'r1';
    live.applied('r1', doc.encode(), doc.assetDirectory);
    await tester.pump();
    (doc.nodes[1]['processor'] as Map)['db'] = 2.0;
    doc.changed();
    await tester.pump(const Duration(milliseconds: 40));
    live.beginApply();
    client.revision = 'r2';
    live.applied('r2', doc.encode(), doc.assetDirectory);
    await tester.pump();
    client.update.completeError(Exception('old revision'));
    await tester.pump();
    expect(live.revision, 'r2');
    live.dispose();
  });

  testWidgets('pending edits flush and dragging sends continuous updates',
      (tester) async {
    final client = PatchTestClient();
    final doc = PatchDocument();
    doc.nodes.insert(1, {
      'id': 3,
      'inputs': [1],
      'processor': {'type': 'gain', 'db': 0.0}
    });
    doc.nodes.last['inputs'] = [3];
    final live = PatchLiveController(doc, client);
    final initial = doc.encode();
    live.beginApply();
    final gain = doc.nodes[1]['processor'] as Map;
    gain['db'] = 1.0;
    doc.changed();
    client.active = true;
    client.revision = 'r1';
    live.applied('r1', initial, doc.assetDirectory);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 40));
    expect(client.updates.length, 1);
    for (var i = 0; i < 10; i++) {
      gain['db'] = i + 2.0;
      doc.changed();
      await tester.pump(const Duration(milliseconds: 10));
    }
    await tester.pump(const Duration(milliseconds: 40));
    expect(client.updates.length, greaterThan(2));
    expect(
        (client.updates.last['document'] as Map)['graph']['nodes'][1]
            ['processor']['db'],
        11.0);
    live.dispose();
  });

  testWidgets('live edits coalesce and structural changes require apply',
      (tester) async {
    final client = PatchTestClient();
    final doc = PatchDocument();
    doc.nodes.insert(1, {
      'id': 3,
      'inputs': [1],
      'processor': {'type': 'gain', 'db': 0.0}
    });
    doc.nodes.last['inputs'] = [3];
    final live = PatchLiveController(doc, client);

    client.active = true;
    client.revision = 'r1';
    live.applied('r1', doc.encode(), doc.assetDirectory);
    await tester.pump();
    final gain = doc.nodes[1]['processor'] as Map;
    gain['db'] = 1.0;
    doc.changed();
    gain['db'] = 2.0;
    doc.changed();
    await tester.pump(const Duration(milliseconds: 40));
    await tester.pump();
    expect(client.updates.length, 1);
    expect(
        (client.updates.single['document'] as Map)['graph']['nodes'][1]
            ['processor']['db'],
        2.0);
    doc.nodes.last['inputs'] = [1];
    doc.changed();
    gain['db'] = 3.0;
    doc.changed();
    await tester.pump(const Duration(milliseconds: 40));
    expect(client.updates.length, 1);
    expect(live.label, 'Reapply rig to hear structural changes');
    client.revision = 'other';
    await live.poll();
    expect(live.revision, isNull);
    live.dispose();
  });

  testWidgets(
      'audio panel applies a draft, returns to board and handles preview failure',
      (tester) async {
    tester.view.physicalSize = const Size(480, 400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final client = PatchTestClient();
    final doc = PatchDocument();
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: SingleChildScrollView(
                child: PatchAudioControls(document: doc, client: client)))));
    await tester.pump();
    await tester.tap(find.text('Apply to guitar'));
    await tester.pump();
    expect(client.active, isTrue);
    expect(client.applied?['document'], doc.data);
    expect(find.textContaining('PATCH LIVE'), findsOneWidget);
    await tester.tap(find.text('Return to pedalboard'));
    await tester.pump();
    expect(client.active, isFalse);
    await tester.tap(find.text('Preview clip'));
    await tester.pump();
    expect(find.textContaining('Preview WAV is missing'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
  });
}
