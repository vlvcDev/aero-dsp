import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

Map<String, dynamic> node(int id, [List<int> inputs = const []]) => {
      'id': id,
      'inputs': inputs,
      'processor': {'type': 'gain', 'db': 0},
    };

void expectNoOverlap(Map<int, Offset> positions) {
  final entries = positions.entries.toList();
  for (var i = 0; i < entries.length; i++) {
    for (var j = i + 1; j < entries.length; j++) {
      final a = entries[i].value;
      final b = entries[j].value;
      expect((a.dx - b.dx).abs() >= 220 || (a.dy - b.dy).abs() >= 132, isTrue,
          reason: '${entries[i].key} overlaps ${entries[j].key}');
    }
  }
}

void main() {
  testWidgets('Fit includes the dry bypass cable around a reverb node',
      (tester) async {
    tester.view.physicalSize = const Size(900, 400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final doc = PatchDocument();
    doc.nodes[1]['inputs'] = [4];
    doc.data['graph']['nodes'] = <dynamic>[
      ...doc.nodes,
      node(3, [1]),
      {
        'id': 4,
        'inputs': [1, 3],
        'processor': {'type': 'mixer'}
      },
    ];
    final original = doc.encode();
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: PatchCanvas(
      document: doc,
      label: (node) => '${node['id']}',
      onSelect: (_) {},
      onConnect: (_, __) {},
      onMoveStart: () {},
    ))));
    await tester.tap(find.byTooltip('Fit patch'));
    await tester.pumpAndSettle();
    final viewer =
        tester.widget<InteractiveViewer>(find.byType(InteractiveViewer));
    final bounds = routePatchCable(doc.positions, 1, 4).getBounds();
    final screenBounds = MatrixUtils.transformRect(
        viewer.transformationController!.value, bounds);
    final viewport =
        Offset.zero & tester.getSize(find.byType(InteractiveViewer));
    expect(viewport.contains(screenBounds.topLeft), isTrue);
    expect(viewport.contains(screenBounds.bottomRight), isTrue);
    expect(bounds.height, greaterThan(20));
    expect(doc.encode(), original);
    expect(tester.takeException(), isNull);
  });

  test('empty patch has no positions', () {
    expect(arrangePatchNodes([]), isEmpty);
  });

  test('scrambled chain follows signal order and is deterministic', () {
    final nodes = [
      node(4, [3]),
      node(2, [1]),
      node(1),
      node(3, [2])
    ];
    final layout = arrangePatchNodes(nodes);
    expect(layout.keys.toSet(), {1, 2, 3, 4});
    for (var id = 1; id < 4; id++) {
      expect(layout[id]!.dx, lessThan(layout[id + 1]!.dx));
      expect(layout[id]!.dy, layout[id + 1]!.dy);
    }
    expect(arrangePatchNodes(nodes.reversed.toList()), layout);
    expectNoOverlap(layout);
  });

  test('parallel branches occupy separate rows and merge downstream', () {
    final layout = arrangePatchNodes([
      node(4, [2, 3]),
      node(3, [1]),
      node(1),
      node(2, [1])
    ]);
    expect(layout[1]!.dx, lessThan(layout[2]!.dx));
    expect(layout[2]!.dx, layout[3]!.dx);
    expect(layout[2]!.dy, isNot(layout[3]!.dy));
    expect(layout[4]!.dx, greaterThan(layout[2]!.dx));
    expectNoOverlap(layout);
  });

  test('long chains wrap after six depths without overlapping', () {
    final nodes = [
      for (var id = 1; id <= 9; id++) node(id, id == 1 ? [] : [id - 1])
    ];
    final layout = arrangePatchNodes(nodes);
    expect(layout.length, 9);
    expect(layout[6]!.dx, greaterThan(layout[1]!.dx));
    expect(layout[7]!.dx, layout[1]!.dx);
    expect(layout[7]!.dy, greaterThan(layout[1]!.dy));
    expect(layout[8]!.dx, greaterThan(layout[7]!.dx));
    expectNoOverlap(layout);
  });

  test('disconnected nodes, missing inputs and cycles are included', () {
    final nodes = [
      node(4, [5]),
      node(2, [999]),
      node(5, [4]),
      node(1),
      node(3, [1])
    ];
    final layout = arrangePatchNodes(nodes);
    expect(layout.keys.toSet(), {1, 2, 3, 4, 5});
    expect(arrangePatchNodes(nodes.reversed.toList()), layout);
    expectNoOverlap(layout);
  });

  testWidgets('Fit patch arranges nodes and undo restores their positions',
      (tester) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final doc = PatchDocument();
    doc.positions[1] = const Offset(500, 400);
    doc.positions[2] = const Offset(80, 90);
    final before = Map<int, Offset>.from(doc.positions);
    final graph = doc.encode();
    await tester.pumpWidget(
        MaterialApp(home: Scaffold(body: PatchEditor(document: doc))));
    await tester.tap(find.byTooltip('Fit patch'));
    await tester.pumpAndSettle();
    expect(doc.positions, isNot(before));
    expect(doc.positions[1]!.dx, lessThan(doc.positions[2]!.dx));
    expect(doc.encode(), graph);
    await tester.tap(find.byTooltip('Undo'));
    await tester.pumpAndSettle();
    expect(doc.positions, before);
    expect(doc.encode(), graph);
    expect(tester.takeException(), isNull);
  });

  testWidgets('Fit patch keeps a tall draft within the viewport',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final doc = PatchDocument();
    doc.data['graph']['nodes'] = [for (var id = 1; id <= 64; id++) node(id)];
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
        body: PatchCanvas(
          document: doc,
          label: (node) => 'Node ${node['id']}',
          onSelect: (_) {},
          onConnect: (_, __) {},
          onMoveStart: () {},
        ),
      ),
    ));
    await tester.tap(find.byTooltip('Fit patch'));
    await tester.pumpAndSettle();
    final viewport = tester.getRect(find.byType(InteractiveViewer));
    for (var id = 1; id <= 64; id++) {
      final rect = tester.getRect(find.byKey(ValueKey('patch-node-$id')));
      expect(viewport.contains(rect.topLeft), isTrue, reason: 'node $id top');
      expect(viewport.contains(rect.bottomRight), isTrue,
          reason: 'node $id bottom');
    }
    expect(tester.takeException(), isNull);
  });
}
