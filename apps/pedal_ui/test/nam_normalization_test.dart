import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  test('normalization flags default off for old snapshots', () {
    final old = EngineSnapshot.fromJson({});
    expect(old.pedalNormalizeEnabled, isFalse);
    expect(old.ampNormalizeEnabled, isFalse);
    final current = EngineSnapshot.fromJson(
        {'pedal_normalize_enabled': true, 'amp_normalize_enabled': true});
    expect(current.pedalNormalizeEnabled, isTrue);
    expect(current.ampNormalizeEnabled, isTrue);
  });
  for (final slot in ModelSlot.values) {
    testWidgets('$slot normalization and gain fit a small panel',
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
          onPressed: () => showAeroControlBay<void>(context,
              title: slot == ModelSlot.pre ? 'Drive NAM' : 'Amp NAM',
              icon: Icons.speaker,
              heightFactor: .95,
              cartridge: true,
              child: SlotControlsSheet(
                slot: slot,
                snapshot: EngineSnapshot.fromJson(
                    {'pre_model': '/a.nam', 'model': '/a.nam'}),
                catalog: _Catalog(),
                onCaptureSelected: (_) async {},
                onChanged: (key, value) async => changes[key] = value,
                onChangeTone: () {},
              )),
          child: const Text('Open'),
        ),
      ))));
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(find.text('NORMALIZE OFF'), findsOneWidget);
      await tester.tap(find.byType(Switch).first);
      await tester.pumpAndSettle();
      expect(
          changes[slot == ModelSlot.pre
              ? 'pedal_normalize_enabled'
              : 'amp_normalize_enabled'],
          1);
      expect(find.text('NORMALIZE ON'), findsOneWidget);
      await tester.scrollUntilVisible(
          find.text(slot == ModelSlot.pre ? 'DRIVE' : 'VOLUME'), 100,
          scrollable: find.byType(Scrollable).first);
      await tester.pumpAndSettle();
      final sliders = tester.widgetList<Slider>(find.byType(Slider)).toList();
      final slider = slot == ModelSlot.pre
          ? sliders.first
          : sliders.firstWhere((s) => s.max == 24);
      expect(slider.min, -24);
      expect(slider.max, 24);
      expect(slider.value, 0);
      slider.onChanged!(-24);
      slider.onChangeEnd!(-24);
      await tester.pumpAndSettle();
      expect(
          changes[slot == ModelSlot.pre ? 'pedal_drive_db' : 'amp_volume_db'],
          -24);
      expect(tester.takeException(), isNull);
    });
  }
}

class _Catalog extends CatalogClient {
  _Catalog() : super(socketPath: '/tmp/nam-normalization-test.sock');
  @override
  Future<List<CatalogModel>> listModels() async => const [
        CatalogModel(
            name: 'A',
            path: '/a.nam',
            sizeBytes: 1,
            source: 'test',
            favorite: false,
            captureGroup: 'group'),
      ];
}
