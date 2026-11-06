import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

class PatchTestClient extends ControlClient {
  bool active = false;
  Map<String, Object?>? applied;
  @override
  Future<Map<String, dynamic>> patchRequest(String type,
      [Map<String, Object?> payload = const {}]) async {
    if (type == 'patch_status') {
      return {
        'available': true,
        'active': active,
        'faulted': false,
        'switching': false,
        'sample_rate': 48000
      };
    }
    if (type == 'apply_patch') {
      applied = payload;
      active = true;
    }
    if (type == 'deactivate_patch') active = false;
    if (type == 'render_patch') throw Exception('Preview WAV is missing');
    return {'queued': true};
  }
}

void main() {
  testWidgets(
      'audio panel applies a draft, returns to board and handles preview failure',
      (tester) async {
    tester.view.physicalSize = const Size(480, 400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final client = PatchTestClient();
    final doc = PatchDocument();
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: SingleChildScrollView(
                child: PatchAudioControls(document: doc, client: client)))));
    await tester.pump();
    await tester.tap(find.text('Apply to guitar'));
    await tester.pump();
    expect(client.active, isTrue);
    expect(client.applied?['document'], doc.data);
    expect(find.textContaining('PATCH LIVE'), findsOneWidget);
    await tester.tap(find.text('Return to pedalboard'));
    await tester.pump();
    expect(client.active, isFalse);
    await tester.tap(find.text('Preview clip'));
    await tester.pump();
    expect(find.textContaining('Preview WAV is missing'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
  });
}
