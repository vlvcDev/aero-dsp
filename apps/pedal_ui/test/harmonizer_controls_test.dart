import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  for (final size in [const Size(800, 600), const Size(480, 320)]) {
    testWidgets('HARMONY shows key, scale and interval by name at $size',
        (tester) async {
      tester.view.physicalSize = size;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: EffectControlsSheet(
          effect: EffectKind.harmonizer,
          snapshot: EngineSnapshot.fromJson({
            'harmonizer_key': 11,
            'harmonizer_scale': 2,
            'harmonizer_interval': -2,
          }),
          onChanged: (key, value) async {},
        )),
      ));
      await tester.pumpAndSettle();
      expect(find.text('B'), findsOneWidget);
      expect(find.text('HARM MINOR'), findsOneWidget);
      expect(find.text('3RD DOWN'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('delay and reverb expose DUCK', (tester) async {
    for (final effect in [EffectKind.delay, EffectKind.reverb]) {
      await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: EffectControlsSheet(
          key: ValueKey(effect),
          effect: effect,
          snapshot: EngineSnapshot.fromJson({'delay_duck': .5}),
          onChanged: (key, value) async {},
        )),
      ));
      await tester.pumpAndSettle();
      expect(find.text('DUCK'), findsOneWidget);
    }
  });
}
