import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  for (final dropA in [false, true]) {
    testWidgets(
        'seven-string ${dropA ? 'Drop A' : 'Standard B'} guided sequence',
        (tester) async {
      tester.view.physicalSize = const Size(480, 640);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final meters = StreamController<MeterSnapshot>();
      addTearDown(meters.close);
      await tester.pumpWidget(MaterialApp(
        home: Scaffold(body: TunerSheet(meters: meters.stream)),
      ));

      Future<void> select(String text) async {
        await tester.ensureVisible(find.text(text));
        await tester.tap(find.text(text));
        await tester.pump();
      }

      await select(dropA ? '7-string Drop A' : '7-string Standard B');
      final notes = [dropA ? 'A1' : 'B1', 'E2', 'A2', 'D3', 'G3', 'B3', 'E4'];
      final frequencies = [
        dropA ? 55.0 : 61.7354,
        82.4069,
        110.0,
        146.8324,
        195.9977,
        246.9417,
        329.6276
      ];
      for (var i = 0; i < notes.length; i++) {
        expect(find.textContaining('Tune ${notes[i]} first'), findsOneWidget);
        meters.add(MeterSnapshot(
          inputDb: -18,
          outputDb: -18,
          clipped: false,
          cpuPercent: 0,
          xruns: 0,
          tunerHz: frequencies[i],
          tunerConfidence: .95,
        ));
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 1050));
      }
      expect(find.text('ALL STRINGS TUNED'), findsOneWidget);
      await select('1 ${notes.first}');
      expect(find.textContaining('Tune ${notes.first} first'), findsOneWidget);
      await select('7 E4');
      await select('Standard');
      expect(find.textContaining('Tune E2 first'), findsOneWidget);
      expect(find.text('7 E4'), findsNothing);
      expect(find.text('6 E4'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }
}
