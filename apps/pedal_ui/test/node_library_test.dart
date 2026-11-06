import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  test('saved nodes retain private asset copies after originals are removed',
      () {
    final root = Directory.systemTemp.createTempSync('aero-assets-');
    addTearDown(() => root.deleteSync(recursive: true));
    final file = File('${root.path}/capture.nam')
      ..writeAsStringSync('capture bytes');
    final doc = PatchDocument()..assetDirectory = root.path;
    doc.nodes[1]['inputs'] = [3];
    doc.nodes.add({
      'id': 3,
      'inputs': [1],
      'processor': {'type': 'external'}
    });
    doc.processors['3'] = {
      'type': 'nam',
      'path': 'capture.nam',
      'input': 'average'
    };
    final saved = CustomNodeLibrary(Directory('${root.path}/library'))
        .save(doc, 'Capture');
    file.deleteSync();
    expect(
        File('${saved.assetDirectory}/${saved.document['processors']['3']['path']}')
            .readAsStringSync(),
        'capture bytes');
    expect(saved.toProcessor().containsKey('version'), isFalse);
  });

  testWidgets('inserted custom node updates explicitly and can undo',
      (tester) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final root = Directory.systemTemp.createTempSync('aero-library-ui-');
    addTearDown(() => root.deleteSync(recursive: true));
    final library = CustomNodeLibrary(root);
    final child = PatchDocument();
    final first = library.save(child, 'Soft voice');
    final doc = PatchDocument();
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(body: PatchEditor(document: doc, library: library))));
    await tester.tap(find.text('My Nodes'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Soft voice'));
    await tester.pumpAndSettle();
    expect(doc.processors['3']['revision'], 1);
    child.nodes[1]['inputs'] = [3];
    child.nodes.add({
      'id': 3,
      'inputs': [1],
      'processor': {'type': 'gain', 'db': -6}
    });
    library.save(child, 'Soft voice', id: first.id);
    expect(doc.processors['3']['document']['graph']['nodes'].length, 2);
    // Finish wiring the draft so update validation can accept it.
    doc.nodes.last['inputs'] = [1];
    doc.nodes[1]['inputs'] = [3];
    await tester.tap(find.text('Update from library'));
    await tester.pumpAndSettle();
    expect(doc.processors['3']['revision'], 2);
    await tester.tap(find.byTooltip('Undo'));
    await tester.pump();
    expect(doc.processors['3']['revision'], 1);
    expect(tester.takeException(), isNull);
  });
  test(
      'library revisions preserve inserted snapshots across saves and restarts',
      () {
    final directory = Directory.systemTemp.createTempSync('aero-custom-node-');
    addTearDown(() => directory.deleteSync(recursive: true));
    final library = CustomNodeLibrary(directory);
    final draft = PatchDocument();
    final first = library.save(draft, 'My sound');
    final instance = CustomNodeRevision.fromJson(first.toJson());
    draft.nodes[1]['inputs'] = [3];
    draft.nodes.add({
      'id': 3,
      'inputs': [1],
      'processor': {'type': 'gain', 'db': -6}
    });
    final second = library.save(draft, 'My sound', id: first.id);
    expect(instance.document['graph']['nodes'].length, 2);
    expect(second.revision, 2);
    final reopened = CustomNodeLibrary(directory).revisions();
    expect(reopened.length, 2);
    final updated = instance.updatedFrom(reopened.last);
    expect(updated.document['graph']['nodes'].length, 3);
    expect(instance.document['graph']['nodes'].length, 2);
    updated.document.clear();
    expect(updated.document['version'], 1);
    expect(() => library.save(draft, 'Bad', id: '../escape'),
        throwsFormatException);
  });
}
