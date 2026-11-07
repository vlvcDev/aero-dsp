import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  test('exposed gain uses the instance snapshot after saving and reopening',
      () {
    final child = PatchDocument();
    child.nodes.add({
      'id': 3,
      'inputs': [1],
      'processor': {'type': 'gain', 'db': -6}
    });
    child.nodes[1]['inputs'] = [3];
    final directory = Directory.systemTemp.createTempSync('node-macro-');
    addTearDown(() => directory.deleteSync(recursive: true));
    final library = CustomNodeLibrary(directory);
    library
        .save(child, 'Boost', macros: [const NodeMacro(3, 'db', 'Loudness')]);
    final revision = library.latest().single;
    final one = revision.toProcessor();
    final two = revision.toProcessor();
    macroSliders(one, (map, key, label, min, max,
        {integer = false, percent = false}) {
      expect(label, 'Loudness');
      map[key] = 8.0;
      return const SizedBox();
    }).toList();
    expect(one['document']['graph']['nodes'][2]['processor']['db'], 8);
    expect(two['document']['graph']['nodes'][2]['processor']['db'], -6);
    expect(
        library.latest().single.document['graph']['nodes'][2]['processor']
            ['db'],
        -6);
    expect(
        () => library.save(child, 'Invalid',
            macros: [const NodeMacro(10, 'db', 'Oops')]),
        throwsFormatException);
  });
  testWidgets('save a named amp macro, insert it, turn it and undo',
      (tester) async {
    tester.view.physicalSize = const Size(1500, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final directory = Directory.systemTemp.createTempSync('edge-amp-macro-');
    addTearDown(() => directory.deleteSync(recursive: true));
    final library = CustomNodeLibrary(directory);
    final doc = PatchDocument();
    doc.nodes.add({
      'id': 3,
      'inputs': [1],
      'processor': {'type': 'external'}
    });
    doc.nodes[1]['inputs'] = [3];
    doc.processors['3'] = {
      'type': 'edge_amp',
      'drive_db': 12.0,
      'tone': 0.55,
      'touch': 0.55,
      'master_db': -6.0
    };
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(body: PatchEditor(document: doc, library: library))));
    await tester.tap(find.text('Save as Custom Node'));
    await tester.pumpAndSettle();
    final dialog = find.byType(AlertDialog);
    await tester.enterText(
        find.descendant(of: dialog, matching: find.byType(TextField)).first,
        'My edge amp');
    await tester.tap(find
        .descendant(
            of: find.byType(MacroPicker), matching: find.byType(InkWell))
        .first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('3 · edge_amp · Preamp drive (dB)').last);
    await tester.pumpAndSettle();
    await tester.enterText(
        find.descendant(
            of: find.byType(MacroPicker), matching: find.byType(TextField)),
        'Breakup');
    await tester.tap(find.widgetWithText(FilledButton, 'Save'));
    await tester.pumpAndSettle();
    expect(library.latest().single.macros.single.label, 'Breakup');
    await tester.tap(find.text('My Nodes'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('My edge amp'));
    await tester.pumpAndSettle();
    final label = find.textContaining('Breakup:');
    final row = find.ancestor(of: label, matching: find.byType(Column)).first;
    final slider =
        find.descendant(of: row, matching: find.byType(Slider)).first;
    await tester.ensureVisible(slider);
    await tester.drag(slider, const Offset(55, 0));
    await tester.pumpAndSettle();
    num instanceDrive() =>
        doc.processors['4']['document']['processors']['3']['drive_db'] as num;
    expect(instanceDrive(), greaterThan(12));
    expect(library.latest().single.document['processors']['3']['drive_db'], 12);
    await tester.tap(find.byTooltip('Undo'));
    await tester.pumpAndSettle();
    expect(instanceDrive(), 12);
    expect(tester.takeException(), isNull);
  });
}
