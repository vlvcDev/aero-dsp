import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

List<List<int>> frets(List<List<FretNote>> shape) =>
    [for (final s in shape) s.map((n) => n.fret).toList()];

List<int> tab(List<ShredNote> notes) => notes.map((n) => n.fret).toList();

void main() {
  test('shapes and patterns match the lesson tab (B minor, 7th fret)', () {
    const b = PentatonicNeck(11);
    expect(frets(const PentatonicNeck(9).shape(1)), [
      [5, 8], [5, 7], [5, 7], [5, 7], [5, 8], [5, 8] //
    ]);
    expect(frets(b.shape(1, threeNps: true)), [
      [7, 10, 12], [7, 9, 12], [7, 9, 12], [7, 9, 11], [7, 10, 12], //
      [7, 10, 12],
    ]);
    expect(frets(b.shape(4, threeNps: true)).take(5), [
      [14, 17, 19], [14, 17, 19], [14, 16, 19], [14, 16, 19], [15, 17, 19] //
    ]);
    expect(frets(b.shape(5, threeNps: true)).take(5), [
      [17, 19, 22], [17, 19, 21], [16, 19, 21], [16, 19, 21], [17, 19, 22] //
    ]);
    expect(buildShredPattern(b, 1, 1)!.length, 31);
    expect(tab(buildShredPattern(b, 2, 1)!).take(12),
        [7, 12, 10, 7, 10, 12, 7, 9, 12, 7, 9, 12]);
    final legato = buildShredPattern(b, 3, 1)!;
    expect(legato.length, 72);
    expect(tab(legato).take(16),
        [7, 10, 12, 7, 9, 12, 9, 7, 12, 10, 7, 10, 12, 7, 9, 12]);
    expect(tab(buildShredPattern(b, 5, 1)!).take(12),
        [7, 10, 12, 7, 9, 12, 7, 9, 12, 7, 9, 12]);
    expect(tab(buildShredPattern(b, 6, 1)!), [
      7, 10, 7, 9, 12, 9, 7, 9, 11, 10, 7, 10, 12, 10, 7, 10, 11, 9, 7, 9, //
      12, 9, 7, 10,
    ]);
    expect(buildShredPattern(b, 7, 1)!.first.fret, 12);
    expect(buildShredPattern(const PentatonicNeck(0), 7, 1), isNull);
    // Group 1 at the A root: E 5-8, A 5-7.
    expect(
        const PentatonicNeck(9)
            .groupInstances(1)
            .first
            .map((n) => n.fret)
            .toList(),
        [5, 8, 5, 7]);
  });

  testWidgets('scale views render at 480 by 320', (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(const MaterialApp(
        home: Scaffold(body: ScaleWorkspace(meters: Stream.empty()))));
    for (final view in ['SHAPES', 'NAVIGATE', 'SHRED']) {
      await tester.tap(find.text(view));
      await tester.pump();
    }
    await tester.tap(find.text('P3'));
    await tester.pump();
    await tester.scrollUntilVisible(
        find.byKey(const ValueKey('shred-tab')), 80,
        scrollable: find.byType(Scrollable).first);
    expect(find.byKey(const ValueKey('shred-tab')), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
