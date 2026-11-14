import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

Future<void> _pumpSheet(
    WidgetTester tester, Map<String, double> changes) async {
  tester.view.physicalSize = const Size(480, 320);
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
  ))));
}

// The settings list is lazy, so the render dropdown only exists once scrolled
// near it. Its label is the last-built form field before the switch.
Finder _renderDropdown(WidgetTester tester) =>
    find.byType(DropdownButtonFormField<int>).last;

void main() {
  testWidgets('NAM oversampling defaults to off and live 2X commits its value',
      (tester) async {
    final changes = <String, double>{};
    await _pumpSheet(tester, changes);

    final live = find.byKey(const Key('live-nam-oversampling'));
    await tester.scrollUntilVisible(live, 100,
        scrollable: find.byType(Scrollable).first);
    expect(tester.widget<SegmentedButton<int>>(live).selected, {1});

    await tester.tap(find.text('2X'));
    await tester.pumpAndSettle();
    expect(changes['nam_oversampling'], 2.0);
    expect(tester.widget<SegmentedButton<int>>(live).selected, {2});

    await tester.scrollUntilVisible(find.text('RENDER NAM OVERSAMPLING'), 100,
        scrollable: find.byType(Scrollable).first);
    final render = _renderDropdown(tester);
    expect(tester.widget<DropdownButtonFormField<int>>(render).initialValue, 0);
    expect(find.descendant(of: render, matching: find.text('Same as live')),
        findsOneWidget);
  });

  testWidgets('render NAM oversampling dropdown commits the chosen multiplier',
      (tester) async {
    final changes = <String, double>{};
    await _pumpSheet(tester, changes);

    await tester.scrollUntilVisible(find.text('RENDER NAM OVERSAMPLING'), 100,
        scrollable: find.byType(Scrollable).first);
    final render = _renderDropdown(tester);
    final button =
        find.descendant(of: render, matching: find.byType(DropdownButton<int>));
    await tester.ensureVisible(button);
    await tester.pumpAndSettle();
    await tester.tap(button);
    await tester.pumpAndSettle();
    await tester.tap(find.text('4x').last);
    await tester.pumpAndSettle();
    expect(changes['render_nam_oversampling'], 4.0);
    expect(tester.takeException(), isNull);
  });
}
