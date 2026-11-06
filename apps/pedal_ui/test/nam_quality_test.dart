import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  testWidgets(
      'NAM sizes scroll on small screens and failed changes retain the selection',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final changes = <String, double>{};
    var fail = false;
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: AudioQualitySheet(
      snapshot: EngineSnapshot.fromJson({}),
      onChanged: (key, value) async {
        if (fail) throw Exception('Cannot prepare model');
        changes[key] = value;
      },
      onMatch: () async => 0,
    ))));
    final live = find.byKey(const Key('live-nam-size'));
    await tester.scrollUntilVisible(live, 100,
        scrollable: find.byType(Scrollable).first);
    expect(tester.widget<SegmentedButton<int>>(live).selected, {1});
    await tester.tap(find.text('LITE'));
    await tester.pumpAndSettle();
    expect(changes['nam_quality'], 0);
    fail = true;
    await tester.tap(find.text('FULL'));
    await tester.pumpAndSettle();
    expect(tester.widget<SegmentedButton<int>>(live).selected, {0});
    fail = false;
    await tester.scrollUntilVisible(find.text('Same as live NAM'), 100,
        scrollable: find.byType(Scrollable).first);
    await tester.tap(find.text('Same as live NAM'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Full NAM').last);
    await tester.pumpAndSettle();
    expect(changes['render_nam_quality'], 1);
    expect(changes['nam_quality'], 0);
    fail = true;
    await tester.tap(find.text('Full NAM').hitTestable());
    await tester.pumpAndSettle();
    await tester.tap(find.text('Lite NAM').last);
    await tester.pumpAndSettle();
    expect(find.text('Full NAM').hitTestable(), findsOneWidget);
    expect(changes['render_nam_quality'], 1);
    expect(tester.takeException(), isNull);
  });
}
