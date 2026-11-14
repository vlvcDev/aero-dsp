import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

Future<void> openAmpBay(
  WidgetTester tester,
  Map<String, dynamic> values,
  Map<String, double> changes,
) async {
  await tester.pumpWidget(MaterialApp(
      home: Scaffold(
          body: Builder(
    builder: (context) => TextButton(
      onPressed: () => showAeroControlBay<void>(
        context,
        title: 'Amp NAM',
        icon: Icons.speaker,
        heightFactor: .95,
        cartridge: true,
        child: SlotControlsSheet(
          slot: ModelSlot.amp,
          snapshot: EngineSnapshot.fromJson(values),
          onChanged: (key, value) async {
            changes[key] = value;
          },
          onChangeTone: () {},
        ),
      ),
      child: const Text('Open'),
    ),
  ))));
  await tester.tap(find.text('Open'));
  await tester.pumpAndSettle();
}

// TIGHT is the fifth card in the horizontal bank, so it is built only after
// the bank is scrolled sideways.
Future<void> revealTight(WidgetTester tester) async {
  final bank = find.byWidgetPredicate(
      (w) => w is Scrollable && w.axisDirection == AxisDirection.right);
  tester.state<ScrollableState>(bank.first).position.jumpTo(300);
  await tester.pumpAndSettle();
  expect(find.text('TIGHT'), findsOneWidget);
}

void main() {
  testWidgets('amp tight bay fits 480 by 320', (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await openAmpBay(tester, {'amp_tight_hz': 120}, {});
    expect(
        find.text(
            'Pre-amp low cut: trims lows before the amp\'s gain stage for tighter high-gain palm mutes. 100–150 Hz is a classic tight-boost setting.'),
        findsOneWidget);
    await revealTight(tester);
    expect(find.text('+120.0 Hz'), findsOneWidget);
    expect(find.text('TIGHT'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  test('amp tight defaults to OFF and preserves saved values', () {
    expect(const EngineSnapshot.disconnected().ampTightHz, 0);
    expect(EngineSnapshot.fromJson({}).ampTightHz, 0);
    expect(EngineSnapshot.fromJson({'amp_tight_hz': 125}).ampTightHz, 125);
  });

  testWidgets('amp tight defaults to OFF and toggles amp_tight_hz',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final changes = <String, double>{};
    await openAmpBay(tester, {}, changes);
    await revealTight(tester);
    expect(find.text('TIGHT'), findsOneWidget);
    expect(find.text('OFF'), findsOneWidget);
    await tester.ensureVisible(find.text('TURN ON'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('TURN ON'));
    await tester.pump();
    expect(changes['amp_tight_hz'], 120);
    expect(find.text('TURN OFF'), findsOneWidget);
    await tester.ensureVisible(find.text('TURN OFF'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('TURN OFF'));
    await tester.pump();
    expect(changes['amp_tight_hz'], 0);
    expect(tester.takeException(), isNull);
  });

  testWidgets('amp tight slider sends a frequency in range', (tester) async {
    tester.view.physicalSize = const Size(480, 520);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final changes = <String, double>{};
    await openAmpBay(tester, {'amp_tight_hz': 150}, changes);
    await revealTight(tester);
    expect(find.text('+150.0 Hz'), findsOneWidget);
    final slider = tester.widget<Slider>(
        find.byWidgetPredicate((w) => w is Slider && w.max == 300));
    expect(slider.min, 20);
    slider.onChanged!(100);
    slider.onChangeEnd!(100);
    await tester.pump();
    expect(changes['amp_tight_hz'], 100);
    expect(tester.takeException(), isNull);
  });
}
