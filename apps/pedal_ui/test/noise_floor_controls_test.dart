import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

Future<void> _pumpSheet(
  WidgetTester tester, {
  required Map<String, double> changes,
  Future<({double squelchThresholdDb, double gateThresholdDb})> Function()?
      onLearnFloor,
}) async {
  tester.view.physicalSize = const Size(480, 3000);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(MaterialApp(
      home: Scaffold(
          body: AudioQualitySheet(
    snapshot: EngineSnapshot.fromJson({}),
    onChanged: (key, value) async {
      changes[key] = value;
    },
    onMatch: () async => 0,
    onLearnFloor: onLearnFloor,
  ))));
}

void main() {
  testWidgets('noise floor defaults to squelch on at -105 dB', (tester) async {
    await _pumpSheet(tester, changes: <String, double>{});
    final tile = tester.widget<SwitchListTile>(
        find.widgetWithText(SwitchListTile, 'Squelch input noise floor'));
    expect(tile.value, isTrue);
    expect(find.text('-105.0 dB'), findsOneWidget);
  });

  testWidgets('toggling squelch commits 0 and LEARN shows busy until done',
      (tester) async {
    final changes = <String, double>{};
    final learn =
        Completer<({double squelchThresholdDb, double gateThresholdDb})>();
    var learnCalls = 0;
    await _pumpSheet(tester, changes: changes, onLearnFloor: () {
      learnCalls += 1;
      return learn.future;
    });
    await tester.tap(find.text('Squelch input noise floor'));
    await tester.pumpAndSettle();
    expect(changes['squelch_enabled'], 0.0);
    await tester.tap(find.text('LEARN NOISE FLOOR'));
    await tester.pump();
    expect(learnCalls, 1);
    expect(find.text("LISTENING… DON'T PLAY"), findsOneWidget);
    expect(
        tester
            .widget<FilledButton>(
                find.widgetWithText(FilledButton, "LISTENING… DON'T PLAY"))
            .onPressed,
        isNull);

    learn.complete((squelchThresholdDb: -96.0, gateThresholdDb: -88.0));
    await tester.pumpAndSettle();
    expect(find.text('LEARN NOISE FLOOR'), findsOneWidget);
    expect(find.text('-96.0 dB'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('gate threshold slider accepts -120 dB', (tester) async {
    tester.view.physicalSize = const Size(800, 600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
          body: EffectControlsSheet(
        effect: EffectKind.gate,
        snapshot: EngineSnapshot.fromJson({'gate_threshold_db': -120}),
        onChanged: (key, value) async {},
      )),
    ));
    await tester.pumpAndSettle();

    final threshold = tester.widget<Slider>(find.byType(Slider).first);
    expect(threshold.min, -120);
    expect(threshold.value, -120);
    expect(tester.takeException(), isNull);
  });
}
