import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  testWidgets(
      'Drive alignment shows the measured delay and rolls back failed switches',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final changes = <String, double>{};
    var fail = false;
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: SlotControlsSheet(
      slot: ModelSlot.pre,
      snapshot: EngineSnapshot.fromJson({
        'sample_rate': 48000,
        'pedal_align_enabled': true,
        'pedal_alignment': {'status': 'accepted', 'delay_samples': 37},
      }),
      onChanged: (key, value) async {
        if (fail) throw Exception('Engine unavailable');
        changes[key] = value;
      },
      onChangeTone: () {},
    ))));
    expect(find.textContaining('+37 samples (0.77 ms)'), findsOneWidget);
    await tester.tap(find.byType(Switch));
    await tester.pumpAndSettle();
    expect(changes['pedal_align_enabled'], 0);
    expect(find.textContaining('Off — original dry blend'), findsOneWidget);
    fail = true;
    await tester.tap(find.byType(Switch));
    await tester.pumpAndSettle();
    expect(tester.widget<Switch>(find.byType(Switch)).value, isFalse);
    expect(tester.takeException(), isNull);
  });
  testWidgets('uncertain and inverted captures never promise correction',
      (tester) async {
    for (final status in ['uncertain', 'inverted', 'accepted']) {
      await tester.pumpWidget(MaterialApp(
          home: Scaffold(
              body: DryWetAlignmentControl(
        key: ValueKey(status),
        snapshot: EngineSnapshot.fromJson({
          'pedal_alignment': {'status': status, 'delay_samples': 0}
        }),
        onChanged: (_, __) async {},
      ))));
      expect(
          find.textContaining(
              status == 'accepted' ? 'already aligned' : 'left unchanged'),
          findsOneWidget);
      expect(tester.takeException(), isNull);
    }
  });
}
