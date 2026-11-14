import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  test('catalog models parse gain and cab metadata, null when absent', () {
    final tagged = CatalogModel.fromJson({
      'name': 'Prog_Lead.nam',
      'path': '/prog.nam',
      'size_bytes': 1024,
      'gain_db': 64,
      'includes_cab': true,
    });
    expect(tagged.gainDb, 64);
    expect(tagged.includesCab, isTrue);
    final plain = CatalogModel.fromJson({
      'name': 'Plain.nam',
      'path': '/plain.nam',
      'size_bytes': 1024,
    });
    expect(plain.gainDb, isNull);
    expect(plain.includesCab, isNull);
  });

  testWidgets('cab double-filter warning appears only with cab on and taps off',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    const warning = 'Capture includes a cab — IR off?';
    final cabOffRequests = <bool>[];

    Future<void> show(Map<String, dynamic> values) async {
      await tester.pumpWidget(MaterialApp(
          home: Scaffold(
              body: Pedalboard(
        key: ValueKey(values.toString()),
        snapshot: EngineSnapshot.fromJson(values),
        onSlotOpen: (_) {},
        onSlotToggle: (_, __) async {},
        onCabinetOpen: () {},
        onCabinetToggle: (bypassed) async => cabOffRequests.add(bypassed),
        onControlChanged: (_, __) async {},
        onEffectOpen: (_) {},
        onEffectToggle: (_, __) async {},
        onLooperOpen: () {},
        onAddPedal: () {},
      ))));
      await tester.pumpAndSettle();
    }

    await show({'amp_includes_cab': true, 'cabinet_enabled': false});
    expect(find.text(warning), findsNothing);
    await show({'amp_includes_cab': false, 'cabinet_enabled': true});
    expect(find.text(warning), findsNothing);
    await show({'amp_includes_cab': true, 'cabinet_enabled': true});
    expect(find.text(warning), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.tap(find.text(warning));
    await tester.pump();
    expect(cabOffRequests, [true]);
    expect(tester.takeException(), isNull);
  });
}
