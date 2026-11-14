import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  testWidgets('transpose slider and quality edit the inspector',
      (tester) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final doc = PatchDocument();
    doc.nodes[1]['inputs'] = [3];
    doc.nodes.add({
      'id': 3,
      'inputs': [1],
      'processor': {'type': 'external'}
    });
    doc.processors['3'] = {
      'type': 'transpose',
      'semitones': -2,
      'quality': 'fast'
    };
    await tester.pumpWidget(
        MaterialApp(home: Scaffold(body: PatchEditor(document: doc))));
    await tester.tap(find.byKey(const ValueKey('patch-handle-3')));
    await tester.pumpAndSettle();
    expect(find.textContaining('low E becomes D'), findsOneWidget);
    expect(find.textContaining('42.7 ms'), findsOneWidget);
    await tester.drag(find.byType(Slider).last, const Offset(40, 0));
    await tester.pumpAndSettle();
    expect(doc.processors['3']['semitones'], isA<int>());
    await tester.tap(find.text('Fast'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Smooth').last);
    await tester.pumpAndSettle();
    expect(doc.processors['3']['quality'], 'smooth');
    expect(tester.takeException(), isNull);
    tester.view.physicalSize = const Size(600, 300);
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });

  testWidgets('custom integer macros survive edits and undo', (tester) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final child = PatchDocument();
    child.nodes[1]['inputs'] = [4];
    child.nodes.addAll(<Map<String, Object>>[
      {
        'id': 3,
        'inputs': [1],
        'processor': {'type': 'external'}
      },
      {
        'id': 4,
        'inputs': [3],
        'processor': {'type': 'external'}
      },
    ]);
    child.processors['3'] = {
      'type': 'transpose',
      'semitones': -2,
      'quality': 'fast'
    };
    child.processors['4'] = {
      'type': 'bitcrusher',
      'bits': 8,
      'rate_hz': 12000,
      'mix': .5
    };
    final doc = PatchDocument();
    doc.nodes[1]['inputs'] = [3];
    doc.nodes.add({
      'id': 3,
      'inputs': [1],
      'processor': {'type': 'external'}
    });
    doc.processors['3'] = {
      'type': 'custom',
      'name': 'slicer_custom',
      'library_id': 'test',
      'revision': 1,
      'asset_directory': '/tmp',
      'document': child.data,
      'macros': [
        {'node_id': 3, 'parameter': 'semitones', 'label': 'Pitch'},
        {'node_id': 4, 'parameter': 'bits', 'label': 'Bits'},
      ]
    };
    await tester.pumpWidget(
        MaterialApp(home: Scaffold(body: PatchEditor(document: doc))));
    await tester.tap(find.byKey(const ValueKey('patch-handle-3')));
    await tester.pumpAndSettle();
    expect(find.textContaining('SLICER_CUSTOM'), findsWidgets);
    for (var i = 0; i < 2; i++) {
      await tester.drag(find.byType(Slider).at(i), const Offset(30, 0));
      await tester.pumpAndSettle();
    }
    Map settings() => doc.processors['3']['document']['processors'] as Map;
    expect(settings()['3']['semitones'], isA<int>());
    expect(settings()['4']['bits'], isA<int>());
    await tester.tap(find.byTooltip('Undo'));
    await tester.pumpAndSettle();
    expect(settings()['3']['semitones'], isA<int>());
    expect(settings()['4']['bits'], isA<int>());
    expect(doc.validate(), isNull);
    expect(tester.takeException(), isNull);
  });
}
