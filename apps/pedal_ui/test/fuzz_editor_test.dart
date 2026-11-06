import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

PatchDocument fuzzDocument() {
  final doc = PatchDocument();
  doc.nodes[1]['inputs'] = [3];
  doc.nodes.add({
    'id': 3,
    'inputs': [1],
    'processor': {'type': 'external'}
  });
  doc.processors['3'] = {
    'type': 'fuzz',
    'drive_db': 18.0,
    'bias': 0.0,
    'tone': .5,
    'mix': 1.0,
    'output_db': -12.0
  };
  return doc;
}

void main() {
  test('fuzz requires finite values within range and strict fields', () {
    final doc = fuzzDocument();
    expect(doc.validate(), isNull);
    final fuzz = doc.processors['3'] as Map;
    for (final entry in <String, List<double>>{
      'drive_db': [-.1, 36.1],
      'bias': [-1.1, 1.1],
      'tone': [-.1, 1.1],
      'mix': [-.1, 1.1],
      'output_db': [-36.1, 6.1]
    }.entries) {
      final original = fuzz[entry.key];
      for (final invalid in [...entry.value, double.nan, double.infinity]) {
        fuzz[entry.key] = invalid;
        expect(doc.validate(), isNotNull, reason: '${entry.key}: $invalid');
      }
      fuzz.remove(entry.key);
      expect(doc.validate(), isNotNull);
      fuzz[entry.key] = original;
    }
    fuzz['path'] = 'unexpected';
    expect(doc.validate(), isNotNull);
  });
  test('validates fuzz in embedded custom documents', () {
    final child = fuzzDocument();
    final parent = PatchDocument();
    parent.nodes[1]['inputs'] = [3];
    parent.nodes.add({
      'id': 3,
      'inputs': [1],
      'processor': {'type': 'external'}
    });
    parent.processors['3'] = {
      'type': 'custom',
      'name': 'Fuzz',
      'library_id': 'fuzz',
      'revision': 1,
      'asset_directory': '/tmp',
      'document': child.data
    };
    expect(parent.validate(), isNull);
    child.processors['3']['mix'] = 2;
    expect(parent.validate(), isNotNull);
  });
  testWidgets('add, edit and export fuzz', (tester) async {
    tester.view.physicalSize = const Size(1400, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final doc = PatchDocument();
    await tester.pumpWidget(
        MaterialApp(home: Scaffold(body: PatchEditor(document: doc))));
    await tester.tap(find.text('+ ADD NODE'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('FUZZ'));
    await tester.pumpAndSettle();
    expect(doc.processors['3'], {
      'type': 'fuzz',
      'drive_db': 18.0,
      'bias': 0.0,
      'tone': .5,
      'mix': 1.0,
      'output_db': -12.0
    });
    doc.nodes[2]['inputs'] = [1];
    doc.nodes[1]['inputs'] = [3];
    await tester.tap(find.byKey(const ValueKey('patch-handle-3')));
    await tester.pumpAndSettle();
    expect(find.text('Tone · darker to brighter: 50%'), findsOneWidget);
    expect(find.text('Mix · wet: 100%'), findsOneWidget);
    expect(find.text('Bias · asymmetry: 0.0'), findsOneWidget);
    await tester.tapAt(
        tester.getCenter(find.byType(Slider).first) + const Offset(40, 0));
    await tester.pump();
    expect(doc.processors['3']['drive_db'], isNot(18.0));
    await tester.tap(find.text('Export JSON'));
    await tester.pumpAndSettle();
    final field = tester.widget<TextField>(find.byType(TextField).last);
    final exported = jsonDecode(field.controller!.text) as Map;
    expect(exported['processors']['3']['drive_db'],
        doc.processors['3']['drive_db']);
    expect(exported['processors']['3'].containsKey('path'), isFalse);
    expect(tester.takeException(), isNull);
  });
}
