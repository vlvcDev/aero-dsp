import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  test('old snapshots receive musical gate defaults', () {
    final controls =
        EngineSnapshot.fromJson({}).effectControls(EffectKind.gate);
    expect(controls, {
      'gate_threshold_db': -80.0,
      'gate_hold_ms': 20.0,
      'gate_release_ms': 50.0,
      'gate_range_db': 80.0,
    });
  });

  for (final size in [const Size(800, 600), const Size(480, 320)]) {
    testWidgets('gate controls and feel presets fit $size', (tester) async {
      tester.view.physicalSize = size;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final changes = <String, double>{};
      await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: EffectControlsSheet(
          effect: EffectKind.gate,
          snapshot: EngineSnapshot.fromJson({'gate_threshold_db': -42}),
          onChanged: (key, value) async {
            changes[key] = value;
          },
        )),
      ));
      await tester.pumpAndSettle();
      expect(find.text('HOLD'), findsOneWidget);
      await tester.tap(find.text('NATURAL SUSTAIN'));
      await tester.pumpAndSettle();
      expect(changes, {
        'gate_hold_ms': 60.0,
        'gate_release_ms': 150.0,
        'gate_range_db': 80.0,
      });
      expect(changes.containsKey('gate_threshold_db'), isFalse);
      // The horizontal bank keeps all four full-size sliders accessible.
      // Swipe the bank's label area, not a vertical slider's gesture target.
      await tester.drag(find.text('HOLD'), const Offset(-300, 0));
      await tester.pumpAndSettle();
      expect(find.text('REDUCTION').hitTestable(), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  }
}
