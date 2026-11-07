import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  final effects = <String, Map<String, dynamic>>{
    'chorus': {'rate_hz': .8, 'depth': .5, 'mix': .35},
    'flanger': {'rate_hz': .3, 'depth': .55, 'feedback': .25, 'mix': .4},
    'tremolo': {'rate_hz': 4.0, 'depth': .55},
    'octaver': {'mix': .45, 'clean_mode': false},
    'overdrive': {'drive_db': 9.0, 'tone': .5, 'mix': 1.0, 'output_db': -6.0},
  };
  test(
      'effects round trip and enforce required fields and ranges, including nested nodes',
      () {
    for (final effect in effects.entries) {
      final child = PatchDocument();
      child.nodes[1]['inputs'] = [3];
      child.nodes.add({
        'id': 3,
        'inputs': [1],
        'processor': {'type': 'external'}
      });
      child.processors['3'] = {'type': effect.key, ...effect.value};
      expect(child.validate(), isNull, reason: effect.key);
      final imported = PatchDocument()..importJson(child.encode());
      expect(imported.processors['3'], child.processors['3']);
      final parent = PatchDocument();
      parent.nodes[1]['inputs'] = [3];
      parent.nodes.add({
        'id': 3,
        'inputs': [1],
        'processor': {'type': 'external'}
      });
      parent.processors['3'] = {
        'type': 'custom',
        'name': 'Effect',
        'library_id': 'effect',
        'revision': 1,
        'asset_directory': '/tmp',
        'document': child.data
      };
      expect(parent.validate(), isNull);
      final settings = child.processors['3'] as Map;
      for (final key in effect.value.keys) {
        final original = settings.remove(key);
        expect(parent.validate(), isNotNull,
            reason: '${effect.key} missing $key');
        settings[key] = original;
        settings[key] = original is bool ? 'false' : double.infinity;
        expect(parent.validate(), isNotNull,
            reason: '${effect.key} invalid $key');
        settings[key] = original;
      }
      settings['unexpected'] = 1;
      expect(parent.validate(), isNotNull);
    }
  });
  testWidgets('browser adds an editable octaver and exports its settings',
      (tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final doc = PatchDocument();
    await tester.pumpWidget(
        MaterialApp(home: Scaffold(body: PatchEditor(document: doc))));
    await tester.tap(find.text('+ ADD NODE'));
    await tester.pumpAndSettle();
    expect(find.text('Modulation'), findsOneWidget);
    await tester.ensureVisible(find.text('OCTAVER'));
    await tester.tap(find.text('OCTAVER'));
    await tester.pumpAndSettle();
    expect(doc.processors['3'], {'type': 'octaver', ...effects['octaver']!});
    doc.nodes[2]['inputs'] = [1];
    doc.nodes[1]['inputs'] = [3];
    await tester.tap(find.byKey(const ValueKey('patch-handle-3')));
    await tester.pumpAndSettle();
    await tester.tap(find.byType(Switch));
    await tester.pumpAndSettle();
    expect(doc.processors['3']['clean_mode'], isTrue);
    await tester.tap(find.text('Export JSON'));
    await tester.pumpAndSettle();
    final field = tester.widget<TextField>(find.byType(TextField).last);
    expect(jsonDecode(field.controller!.text)['processors']['3']['clean_mode'],
        isTrue);
    expect(tester.takeException(), isNull);
  });
}
