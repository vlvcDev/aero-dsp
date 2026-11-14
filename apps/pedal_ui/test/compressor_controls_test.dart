import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  test('old snapshots receive compressor defaults', () {
    final controls =
        EngineSnapshot.fromJson({}).effectControls(EffectKind.compressor);
    expect(controls, {
      'compressor_threshold_db': -18.0,
      'compressor_ratio': 3.0,
      'compressor_attack_ms': 10.0,
      'compressor_release_ms': 120.0,
      'compressor_mix': 1.0,
      'compressor_level_db': 0.0,
    });
    expect(EngineSnapshot.fromJson({}).compressorAutoMakeup, isTrue);
  });

  testWidgets('SNAP preset sends its compressor values', (tester) async {
    tester.view.physicalSize = const Size(800, 600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final changes = <String, double>{};
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
          body: EffectControlsSheet(
        effect: EffectKind.compressor,
        snapshot: EngineSnapshot.fromJson({}),
        onChanged: (key, value) async {
          changes[key] = value;
        },
      )),
    ));
    await tester.pumpAndSettle();
    await tester.tap(find.text('SNAP'));
    await tester.pumpAndSettle();
    expect(changes, {
      'compressor_threshold_db': -24.0,
      'compressor_ratio': 4.0,
      'compressor_attack_ms': 20.0,
      'compressor_release_ms': 120.0,
      'compressor_mix': 0.7,
    });
  });

  for (final size in [const Size(800, 600), const Size(480, 320)]) {
    testWidgets('compressor controls fit $size', (tester) async {
      tester.view.physicalSize = size;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: EffectControlsSheet(
          effect: EffectKind.compressor,
          snapshot: EngineSnapshot.fromJson({}),
          onChanged: (key, value) async {},
        )),
      ));
      await tester.pumpAndSettle();
      expect(find.text('AUTO MAKEUP'), findsOneWidget);
      // The horizontal bank reaches the last control, LEVEL, by swiping.
      await tester.drag(find.text('THRESHOLD'), const Offset(-600, 0));
      await tester.pumpAndSettle();
      expect(find.text('LEVEL').hitTestable(), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  }
}
