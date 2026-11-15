import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  test('old snapshots receive DE-MUD defaults', () {
    final snapshot = EngineSnapshot.fromJson({});
    expect(snapshot.demudEnabled, isFalse);
    expect(snapshot.demudVisible, isFalse);
    expect(snapshot.effectControls(EffectKind.demud), {
      'demud_threshold_db': -4.0,
      'demud_frequency_hz': 250.0,
      'demud_range_db': 6.0,
    });
  });

  testWidgets('DE-MUD controls fit 480x320 and 800x600', (tester) async {
    for (final size in [const Size(800, 600), const Size(480, 320)]) {
      tester.view.physicalSize = size;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: EffectControlsSheet(
          effect: EffectKind.demud,
          snapshot: EngineSnapshot.fromJson({}),
          onChanged: (key, value) async {},
        )),
      ));
      await tester.pumpAndSettle();
      expect(find.text('THRESHOLD').hitTestable(), findsOneWidget);
      expect(find.text('FREQ').hitTestable(), findsOneWidget);
      // The horizontal bank keeps the last slider reachable.
      await tester.drag(find.text('FREQ'), const Offset(-300, 0));
      await tester.pumpAndSettle();
      expect(find.text('MAX CUT').hitTestable(), findsOneWidget);
      expect(tester.takeException(), isNull);
    }
  });

  testWidgets('CUT readout shows the deepest cut from the meters stream',
      (tester) async {
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
          body: EffectControlsSheet(
        effect: EffectKind.demud,
        snapshot: EngineSnapshot.fromJson({}),
        onChanged: (key, value) async {},
        meters:
            Stream.value(MeterSnapshot.fromJson({'demud_reduction_db': 3.4})),
      )),
    ));
    await tester.pumpAndSettle();
    expect(find.text('CUT 3.4 dB'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
