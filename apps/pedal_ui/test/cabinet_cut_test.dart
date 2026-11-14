import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  test('cabinet cuts default to bypass and preserve saved values', () {
    expect(const EngineSnapshot.disconnected().cabinetLowCutHz, 0);
    expect(EngineSnapshot.fromJson({}).cabinetHighCutHz, 0);
    final saved = EngineSnapshot.fromJson({
      'cabinet_low_cut_hz': 125,
      'cabinet_high_cut_hz': 6500,
    });
    expect(saved.cabinetLowCutHz, 125);
    expect(saved.cabinetHighCutHz, 6500);
  });

  testWidgets('cabinet cut toggles send valid on and off values',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final changes = <String, double>{};
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(
      body: CabinetControlsSheet(
        snapshot: EngineSnapshot.fromJson({}),
        onChanged: (key, value) async {
          changes[key] = value;
        },
        onChooseIr: () async {},
        onClear: () async {},
      ),
    )));
    expect(find.text('OFF'), findsNWidgets(2));
    expect(find.textContaining('0.0 Hz'), findsNothing);
    await tester.ensureVisible(find.text('TURN ON').first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('TURN ON').first);
    await tester.pump();
    expect(changes['cabinet_low_cut_hz'], 80);
    expect(find.text('TURN OFF'), findsOneWidget);
    await tester.tap(find.text('TURN OFF'));
    await tester.pump();
    expect(changes['cabinet_low_cut_hz'], 0);
    await tester.tap(find.text('TURN ON').last);
    await tester.pump();
    expect(changes['cabinet_high_cut_hz'], 8000);
    expect(tester.takeException(), isNull);
  });
  testWidgets(
      'saved cuts display their frequencies and sliders enable from off',
      (tester) async {
    final changes = <String, double>{};
    Future<void> show(Map<String, dynamic> values) async {
      await tester.pumpWidget(MaterialApp(
          home: Scaffold(
        body: CabinetControlsSheet(
          key: ValueKey(values.toString()),
          snapshot: EngineSnapshot.fromJson(values),
          onChanged: (key, value) async {
            changes[key] = value;
          },
          onChooseIr: () async {},
          onClear: () async {},
        ),
      )));
    }

    await show({'cabinet_low_cut_hz': 125, 'cabinet_high_cut_hz': 6500});
    expect(find.text('+125.0 Hz'), findsOneWidget);
    expect(find.text('+6500.0 Hz'), findsOneWidget);
    await show({});
    final sliders = tester.widgetList<Slider>(find.byType(Slider)).toList();
    sliders[1].onChanged!(20);
    sliders[1].onChangeEnd!(20);
    sliders[2].onChanged!(1000);
    sliders[2].onChangeEnd!(1000);
    await tester.pump();
    expect(changes['cabinet_low_cut_hz'], 20);
    expect(changes['cabinet_high_cut_hz'], 1000);
    expect(tester.takeException(), isNull);
  });

  testWidgets('cabinet bay remembers cut frequencies at 480 by 320',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final changes = <String, double>{};
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: Builder(
      builder: (context) => TextButton(
          onPressed: () => showAeroControlBay<void>(
                context,
                title: 'Cabinet IR',
                icon: Icons.speaker_outlined,
                heightFactor: .95,
                cartridge: true,
                child: CabinetControlsSheet(
                  snapshot: EngineSnapshot.fromJson(
                      {'cabinet_low_cut_hz': 150, 'cabinet_high_cut_hz': 6500}),
                  onChanged: (key, value) async {
                    changes[key] = value;
                  },
                  onChooseIr: () async {},
                  onClear: () async {},
                ),
              ),
          child: const Text('Open')),
    ))));
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    await tester.drag(find.byType(SingleChildScrollView).last, const Offset(0, -160));
    await tester.pumpAndSettle();
    await tester.tap(find.text('TURN OFF').first);
    await tester.pump();
    expect(changes['cabinet_low_cut_hz'], 0);
    await tester.tap(find.text('TURN ON'));
    await tester.pump();
    expect(changes['cabinet_low_cut_hz'], 150);
    await tester.tap(find.text('TURN OFF').last);
    await tester.pump();
    expect(changes['cabinet_high_cut_hz'], 0);
    await tester.tap(find.text('TURN ON'));
    await tester.pump();
    expect(changes['cabinet_high_cut_hz'], 6500);
    expect(tester.takeException(), isNull);
  });
  testWidgets('loaded cabinet board fits 480 by 320', (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final changes = <String, double>{};
    await tester.pumpWidget(MaterialApp(home: Scaffold(body: Pedalboard(
      snapshot: EngineSnapshot.fromJson({'cabinet_ir': '/cab.wav', 'cabinet_low_cut_hz': 150, 'cabinet_high_cut_hz': 6500}),
      onSlotOpen: (_) {}, onSlotToggle: (_, __) async {},
      onCabinetOpen: () {}, onCabinetToggle: (_) async {},
      onControlChanged: (key, value) async { changes[key] = value; },
      onEffectOpen: (_) {}, onEffectToggle: (_, __) async {},
      onLooperOpen: () {}, onAddPedal: () {},
    ))));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    await tester.drag(find.byType(ListView).first, const Offset(-600, 0));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    tester.view.physicalSize = const Size(480, 520);
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Low cut: tap to turn on or off; drag to adjust frequency'));
    await tester.pump();
    expect(changes['cabinet_low_cut_hz'], 0);
    await tester.tap(find.byTooltip('Low cut: tap to turn on or off; drag to adjust frequency'));
    await tester.pump();
    expect(changes['cabinet_low_cut_hz'], 150);
    expect(tester.takeException(), isNull);
  });

}
