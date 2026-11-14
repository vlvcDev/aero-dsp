import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  test('old snapshots receive DEHISS defaults', () {
    final snapshot = EngineSnapshot.fromJson({});
    expect(snapshot.dehissEnabled, isFalse);
    expect(snapshot.effectControls(EffectKind.dehiss), {
      'dehiss_threshold_db': -70.0,
      'dehiss_frequency_hz': 3000.0,
      'dehiss_range_db': 24.0,
    });
  });

  for (final size in [const Size(800, 600), const Size(480, 320)]) {
    testWidgets('dehiss controls and LEARN fit $size', (tester) async {
      tester.view.physicalSize = size;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: EffectControlsSheet(
          effect: EffectKind.dehiss,
          snapshot: EngineSnapshot.fromJson({}),
          onChanged: (key, value) async {},
          onLearn: () async => -60,
        )),
      ));
      await tester.pumpAndSettle();
      expect(find.text('LEARN'), findsOneWidget);
      expect(find.text('THRESHOLD').hitTestable(), findsOneWidget);
      expect(find.text('ABOVE').hitTestable(), findsOneWidget);
      // The horizontal bank keeps all three full-size sliders accessible.
      await tester.drag(find.text('ABOVE'), const Offset(-300, 0));
      await tester.pumpAndSettle();
      expect(find.text('REDUCTION').hitTestable(), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('LEARN calls the injected learn callback and applies threshold',
      (tester) async {
    final learn = Completer<double>();
    var calls = 0;
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
          body: EffectControlsSheet(
        effect: EffectKind.dehiss,
        snapshot: EngineSnapshot.fromJson({}),
        onChanged: (key, value) async {},
        onLearn: () {
          calls += 1;
          return learn.future;
        },
      )),
    ));
    await tester.pumpAndSettle();

    await tester.tap(find.text('LEARN'));
    await tester.pump();
    expect(calls, 1);
    expect(find.text('LISTENING… DON\'T PLAY'), findsOneWidget);
    final busyButton = tester.widget<FilledButton>(find.ancestor(
        of: find.text('LISTENING… DON\'T PLAY'),
        matching: find.byType(FilledButton)));
    expect(busyButton.onPressed, isNull);

    learn.complete(-58);
    await tester.pumpAndSettle();
    expect(find.text('LEARN'), findsOneWidget);
    expect(find.text('-58.0 dB'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('a failed LEARN shows a snackbar and keeps the threshold',
      (tester) async {
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
          body: EffectControlsSheet(
        effect: EffectKind.dehiss,
        snapshot: EngineSnapshot.fromJson({}),
        onChanged: (key, value) async {},
        onLearn: () async => throw const EngineException('No audio measured'),
      )),
    ));
    await tester.pumpAndSettle();

    await tester.tap(find.text('LEARN'));
    await tester.pumpAndSettle();
    expect(find.textContaining('No audio measured'), findsOneWidget);
    expect(find.text('-70.0 dB'), findsOneWidget);
  });
}
