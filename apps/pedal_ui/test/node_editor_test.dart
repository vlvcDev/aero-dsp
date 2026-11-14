import 'dart:io';
import 'dart:ui' show PointerDeviceKind;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  test('new effects validate strict types, ranges and serialization', () {
    final effects = <Map<String, dynamic>>[
      {'type': 'transpose', 'semitones': -2, 'quality': 'fast'},
      {
        'type': 'envelope_filter',
        'sensitivity': .5,
        'base_hz': 300,
        'resonance': 2,
        'decay_ms': 200,
        'mix': 1
      },
      {'type': 'ring_mod', 'frequency_hz': 80, 'mix': .5},
      {
        'type': 'slicer',
        'bpm': 120,
        'division': 2,
        'pattern': 'pulse',
        'depth': 1
      },
      {'type': 'bitcrusher', 'bits': 8, 'rate_hz': 12000, 'mix': .5},
    ];
    for (final effect in effects) {
      final doc = PatchDocument();
      doc.nodes[1]['inputs'] = [3];
      doc.nodes.add({
        'id': 3,
        'inputs': [1],
        'processor': {'type': 'external'}
      });
      doc.processors['3'] = effect;
      expect(doc.validate(), isNull, reason: effect['type']);
      final restored = PatchDocument()..importJson(doc.encode());
      expect(restored.processors['3'], effect);
      effect['unexpected'] = 1;
      expect(doc.validate(), isNotNull);
      effect.remove('unexpected');
    }
    effects[0]['semitones'] = 1.0;
    effects[3]['division'] = 3;
    effects[4]['bits'] = 8.0;
    for (final effect in [effects[0], effects[3], effects[4]]) {
      final doc = PatchDocument();
      doc.nodes[1]['inputs'] = [3];
      doc.nodes.add({
        'id': 3,
        'inputs': [1],
        'processor': {'type': 'external'}
      });
      doc.processors['3'] = effect;
      expect(doc.validate(), isNotNull);
    }
  });

  test('new cables reject feedback and preserve mixer capacity', () {
    final doc = PatchDocument();
    doc.nodes.add({
      'id': 3,
      'inputs': [1],
      'processor': {'type': 'gain', 'db': 0}
    });
    doc.nodes.add({
      'id': 4,
      'inputs': [3],
      'processor': {'type': 'gain', 'db': 0}
    });
    expect(doc.connectionError(4, 3), contains('Feedback'));
    expect(doc.connectionError(3, 3), contains('Feedback'));
    expect(doc.connectionError(2, 3), contains('socket'));
    expect(doc.connectionError(1, 3), contains('already'));
    expect(doc.connectionError(3, 2), isNull);
    doc.nodes.add({
      'id': 5,
      'inputs': List.generate(8, (i) => i + 10),
      'processor': {'type': 'mixer'}
    });
    expect(doc.connectionError(3, 5), contains('eight'));
  });

  testWidgets('canvas moves nodes, connects sockets and opens controls',
      (tester) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final doc = PatchDocument();
    doc.nodes[1]['inputs'] = <int>[];
    await tester.pumpWidget(
        MaterialApp(home: Scaffold(body: PatchEditor(document: doc))));
    final initial = doc.positions[1]!;
    await tester.drag(
        find.byKey(const ValueKey('patch-handle-1')), const Offset(50, 60),
        kind: PointerDeviceKind.mouse);
    await tester.pump();
    expect(doc.positions[1]!.dx, greaterThan(initial.dx + 20));
    expect(doc.positions[1]!.dy, greaterThan(initial.dy + 20));
    await tester.dragFrom(
        tester.getCenter(find.byKey(const ValueKey('patch-output-1'))),
        tester.getCenter(find.byKey(const ValueKey('patch-input-2'))) -
            tester.getCenter(find.byKey(const ValueKey('patch-output-1'))));
    await tester.pumpAndSettle();
    expect(doc.nodes[1]['inputs'], [1]);
    await tester.tap(find.byTooltip('Undo'));
    await tester.pump();
    expect(doc.nodes[1]['inputs'], isEmpty);
    await tester.tap(find.byKey(const ValueKey('patch-output-1')));
    await tester.tap(find.byKey(const ValueKey('patch-input-2')));
    await tester.pump();
    expect(doc.nodes[1]['inputs'], [1]);
    await tester.tap(find.byTooltip('Fit patch'));
    await tester.pump();
    await tester.tap(find.byTooltip('Zoom out'));
    await tester.pump();
    final beforeZoomedDrag = doc.positions[1]!;
    await tester.drag(
        find.byKey(const ValueKey('patch-handle-1')), const Offset(30, 30));
    await tester.pump();
    expect(doc.positions[1]!.dx, greaterThan(beforeZoomedDrag.dx + 30));
    await tester.tap(find.byKey(const ValueKey('patch-handle-1')));
    await tester.pump();
    expect(find.text('Close controls'), findsOneWidget);
    expect(tester.takeException(), isNull);
    // Canvas layout remains separate from the strict native JSON schema.
    expect(doc.encode(), isNot(contains('positions')));
  });

  test('imports the native renderer example without changing its schema', () {
    final document = PatchDocument();
    document.importJson(
        File('../../config/node-rig-example.json').readAsStringSync());
    expect(document.validate(), isNull);
    expect(document.nodes.length, 6);
    expect(document.processors['3']['type'], 'reverb');
    final restored = PatchDocument()..importJson(document.encode());
    expect(restored.encode(), document.encode());
  });

  test('rejects feedback, disconnected nodes, and malformed imports atomically',
      () {
    final document = PatchDocument();
    final original = document.encode();
    expect(() => document.importJson('{"version":2}'), throwsFormatException);
    expect(document.encode(), original);
    document.nodes.add({
      'id': 3,
      'inputs': [3],
      'processor': {'type': 'gain', 'db': 0}
    });
    expect(document.validate(), contains('unused'));
    document.nodes[1]['inputs'] = [3];
    expect(document.validate(), contains('Feedback'));
    document.nodes[2]['inputs'] = [1];
    expect(document.validate(), isNull);
    document.nodes[2]['processor']['db'] = 25;
    expect(document.validate(), contains('Gain'));
  });

  testWidgets('compact editor can add and undo a node without overflow',
      (tester) async {
    tester.view.physicalSize = const Size(480, 400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final document = PatchDocument();
    await tester.pumpWidget(
        MaterialApp(home: Scaffold(body: PatchEditor(document: document))));
    expect(tester.takeException(), isNull);
    await tester.tap(find.text('+ ADD NODE'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('GAIN'));
    await tester.pumpAndSettle();
    expect(document.nodes.length, 3);
    expect(tester.takeException(), isNull);
    await tester.tap(find.byTooltip('Undo'));
    await tester.pump();
    expect(document.nodes.length, 2);
    expect(document.validate(), isNull);
    await tester.tap(find.byKey(const ValueKey('patch-handle-1')));
    await tester.pump();
    expect(find.text('Close controls'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.tap(find.text('Close controls'));
    await tester.pump();
    expect(find.byKey(const ValueKey('patch-handle-1')), findsOneWidget);
  });
}
