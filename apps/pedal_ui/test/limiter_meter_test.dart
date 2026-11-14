import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  test(
      'reduction fields parse independently and older payloads are unavailable',
      () {
    final snapshot = MeterSnapshot.fromJson({
      'rig_limiter_reduction_db': 2,
      'output_limiter_reduction_db': 4.5,
    });
    expect(snapshot.rigLimiterReductionDb, 2.0);
    expect(snapshot.outputLimiterReductionDb, 4.5);
    expect(MeterSnapshot.fromJson({}).rigLimiterReductionDb, isNull);
    expect(const MeterSnapshot.silent().outputLimiterReductionDb, isNull);
  });

  testWidgets('compact meter displays and clears reductions', (tester) async {
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
      'rig_limiter_reduction_db': 2.0,
      'output_limiter_reduction_db': 4.5,
    }));
    expect(find.text('RIG 2.0 dB'), findsOneWidget);
    expect(find.text('OUT 4.5 dB'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await show(MeterSnapshot.fromJson({
      'patch_active': true,
      'rig_limiter_reduction_db': 2.0,
      'output_limiter_reduction_db': 4.5,
    }));
    expect(find.text('RIG OFF'), findsOneWidget);
    expect(find.text('OUT 4.5 dB'), findsOneWidget);
    await show(MeterSnapshot.fromJson({}));
    expect(find.text('RIG —'), findsOneWidget);
    expect(find.text('OUT —'), findsOneWidget);
    await show(const MeterSnapshot.silent());
    expect(find.text('AUDIO DISABLED'), findsOneWidget);
    expect(find.text('OUT —'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
  testWidgets('narrow large text clears stale values when disabled',
      (tester) async {
    tester.view.physicalSize = const Size(320, 600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    Future<void> show(String status) async {
      await tester.pumpWidget(MaterialApp(
        builder: (context, child) => MediaQuery(
          data: MediaQuery.of(context)
              .copyWith(textScaler: const TextScaler.linear(1.3)),
          child: child!,
        ),
        home: Scaffold(
            body: Align(
                alignment: Alignment.topCenter,
                child: MeterStrip(
                    snapshot: MeterSnapshot.fromJson({
                  'audio_status': status,
                  'rig_limiter_reduction_db': -0.0,
                  'output_limiter_reduction_db': 6.0,
                })))),
      ));
    }

    await show('running');
    expect(find.text('RIG 0.0 dB'), findsOneWidget);
    expect(find.text('OUT 6.0 dB'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await show('disabled');
    expect(find.text('RIG —'), findsOneWidget);
    expect(find.text('OUT —'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
