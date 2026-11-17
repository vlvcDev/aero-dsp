import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

MeterSnapshot _pitch(double hz, {double confidence = .95}) => MeterSnapshot(
      inputDb: -18,
      outputDb: -18,
      clipped: false,
      cpuPercent: 0,
      xruns: 0,
      tunerHz: hz,
      tunerConfidence: confidence,
    );

String _text(WidgetTester tester, String key) =>
    tester.widget<Text>(find.byKey(ValueKey(key))).data!;

void main() {
  test('nearest note and cents for any frequency', () {
    final e = ChromaticReading.fromHz(82.4069);
    expect((e.name, e.octave), ('E', 2));
    expect(e.cents.abs(), lessThan(.1));
    final flatA = ChromaticReading.fromHz(108.75);
    expect((flatA.name, flatA.octave), ('A', 2));
    expect(flatA.cents, closeTo(-19.8, .2));
    final sharpB = ChromaticReading.fromHz(63);
    expect((sharpB.name, sharpB.octave, sharpB.cents.round()), ('B', 1, 35));
    expect(ChromaticReading.fromHz(442, a4: 442).cents.abs(), lessThan(.01));
  });

  for (final size in [const Size(480, 275), const Size(800, 516)]) {
    testWidgets('chromatic tuner follows any note at $size', (tester) async {
      tester.view.physicalSize = size;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final meters = StreamController<MeterSnapshot>();
      addTearDown(meters.close);
      await tester.pumpWidget(MaterialApp(
          home: Scaffold(body: TunerSheet(meters: meters.stream))));

      expect(_text(tester, 'tuner-status'), 'PLAY ANY STRING');
      meters.add(_pitch(82.4069));
      await tester.pump();
      expect(_text(tester, 'tuner-note'), 'E');
      expect(_text(tester, 'tuner-below'), 'D  D#');
      expect(_text(tester, 'tuner-above'), 'F  F#');
      expect(_text(tester, 'tuner-status'), startsWith('IN TUNE'));
      expect(find.bySemanticsLabel('In tune'), findsOneWidget);

      meters.add(_pitch(108.75));
      await tester.pump();
      expect(_text(tester, 'tuner-note'), 'A');
      expect(_text(tester, 'tuner-status'), startsWith('-20 cents'));
      expect(_text(tester, 'tuner-below'), 'G  G#');
      expect(_text(tester, 'tuner-above'), 'A#  B');

      // Low-confidence frames don't change the note; it clears after a hold.
      meters.add(_pitch(300, confidence: .2));
      await tester.pump();
      expect(_text(tester, 'tuner-note'), 'A');
      await tester.pump(const Duration(seconds: 2));
      expect(_text(tester, 'tuner-status'), 'PLAY ANY STRING');
      expect(find.byKey(const ValueKey('tuner-below')), findsNothing);

      await tester.tap(find.byTooltip('Raise reference pitch'));
      await tester.pump();
      expect(_text(tester, 'tuner-reference'), 'A4 441 Hz');
      expect(tester.takeException(), isNull);
    });
  }
}
