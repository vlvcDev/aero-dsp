import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  for (final size in [const Size(480, 320), const Size(800, 600)]) {
    testWidgets('NAM EQ routing, bands and error rollback fit $size',
        (tester) async {
      tester.view.physicalSize = size;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final changes = <String, double>{};
      var fail = false;
      await tester.pumpWidget(MaterialApp(
          home: Scaffold(
              body: NamEqSheet(
        prefix: 'pedal_eq',
        settings: const {},
        onChanged: (key, value) async {
          if (fail) throw Exception('Engine disconnected');
          changes[key] = value;
        },
      ))));
      await tester.tap(find.text('PRE NAM'));
      await tester.pumpAndSettle();
      expect(changes['pedal_eq_pre'], 1);
      fail = true;
      await tester.tap(find.byType(Switch));
      await tester.pumpAndSettle();
      expect(tester.widget<Switch>(find.byType(Switch)).value, isTrue);
      fail = false;
      await tester.scrollUntilVisible(find.text('LOW CUT').hitTestable(), 80,
          scrollable: find.byType(Scrollable).first, continuous: true);
      await tester.pumpAndSettle();
      await tester.tap(find.text('LOW CUT'));
      await tester.pumpAndSettle();
      expect(changes['pedal_eq_band_0_cut'], 1);
      expect(find.text('GAIN'), findsNothing);
      await tester.scrollUntilVisible(find.text('WIDTH / Q'), 80,
          scrollable: find.byType(Scrollable).first, continuous: true);
      await tester.pumpAndSettle();
      final frequency = tester.widget<Slider>(find.byType(Slider).first);
      frequency.onChanged!(frequency.max);
      frequency.onChangeEnd!(frequency.max);
      await tester.pumpAndSettle();
      expect(changes['pedal_eq_band_0_freq_hz'], closeTo(20000, .01));
      await tester.scrollUntilVisible(find.text('RESET EQ').hitTestable(), -100,
          scrollable: find.byType(Scrollable).first, continuous: true);
      await tester.pumpAndSettle();
      await tester.tap(find.text('RESET EQ'));
      await tester.pumpAndSettle();
      expect(changes['pedal_eq_reset'], 1);
      expect(tester.takeException(), isNull);
    });
  }
  testWidgets('head EQ opens from the slot and restores saved settings',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final changes = <String, double>{};
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: SlotControlsSheet(
      slot: ModelSlot.amp,
      snapshot: EngineSnapshot.fromJson({
        'amp_eq': {'enabled': false, 'pre': true}
      }),
      onChanged: (k, v) async => changes[k] = v,
      onChangeTone: () {},
    ))));
    await tester.tap(find.text('6-BAND EQ'));
    await tester.pumpAndSettle();
    expect(tester.widget<Switch>(find.byType(Switch)).value, isFalse);
    await tester.tap(find.byType(Switch));
    await tester.pumpAndSettle();
    expect(changes['amp_eq_enabled'], 1);
    expect(tester.takeException(), isNull);
  });
}
