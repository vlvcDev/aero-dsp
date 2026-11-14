import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  test('realtime priorities default to 0 for empty and older payloads', () {
    final snapshot = MeterSnapshot.fromJson({});
    expect(snapshot.audioRealtimePriority, 0);
    expect(snapshot.dspWorkerRealtimePriority, 0);
    expect(snapshot.realtimeLabel, 'Realtime: off');
    expect(const MeterSnapshot.silent().audioRealtimePriority, 0);
  });

  testWidgets('meter strip shows realtime scheduling state', (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    Future<void> show(MeterSnapshot snapshot) async {
      await tester.pumpWidget(MaterialApp(
          home: Scaffold(
              body: Align(
        alignment: Alignment.topCenter,
        child: MeterStrip(snapshot: snapshot),
      ))));
    }

    await show(MeterSnapshot.fromJson({
      'audio_realtime_priority': 0,
      'dsp_worker_realtime_priority': 0,
    }));
    expect(find.textContaining('Realtime: off'), findsOneWidget);
    expect(tester.takeException(), isNull);

    await show(MeterSnapshot.fromJson({
      'audio_realtime_priority': 70,
      'dsp_worker_realtime_priority': 69,
    }));
    expect(
        find.textContaining('Realtime: FIFO 70 · workers 69'), findsOneWidget);
    expect(find.textContaining('Realtime: off'), findsNothing);
    expect(tester.takeException(), isNull);
  });
}
