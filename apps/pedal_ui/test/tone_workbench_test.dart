import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

class FakeClient extends ControlClient {
  final calls = <Map<String, Object?>>[];
  @override
  Future<Map<String, dynamic>> patchRequest(String type,
      [Map<String, Object?> payload = const {}]) async {
    calls.add({'type': type, ...payload});
    throw StateError('render unavailable');
  }
}

void main() {
  testWidgets(
      'captures independent snapshots, persists them and recalls a winner',
      (tester) async {
    tester.view.physicalSize = const Size(600, 500);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final doc = PatchDocument()..assetDirectory = '/first';
    final client = FakeClient();
    ToneWorkbenchResult? recalled;
    await tester.pumpWidget(MaterialApp(
        home: Builder(
            builder: (context) => Scaffold(
                    body: TextButton(
                  onPressed: () async {
                    recalled = await showDialog<ToneWorkbenchResult>(
                        context: context,
                        builder: (_) => Dialog(
                            child:
                                ToneWorkbench(document: doc, client: client)));
                  },
                  child: const Text('Open'),
                )))));
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Capture draft').first);
    await tester.pump();
    doc.assetDirectory = '/second';
    doc.data['sample_rate'] = 44100;
    await tester.ensureVisible(find.text('Capture draft').last);
    await tester.tap(find.text('Capture draft').last);
    await tester.pump();
    await tester.tap(find.byTooltip('Close'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    expect(find.text('2 nodes • 48000 Hz'), findsOneWidget);
    expect(find.text('2 nodes • 44100 Hz'), findsOneWidget);
    await tester.tap(find.text('Recall to draft').first);
    await tester.pumpAndSettle();
    expect(recalled!.document['sample_rate'], 48000);
    expect(recalled!.assetDirectory, '/first');
    expect(doc.data['sample_rate'], 44100);
    expect(tester.takeException(), isNull);
  });

  testWidgets('uses paired render and reports errors without applying a rig',
      (tester) async {
    tester.view.physicalSize = const Size(900, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final doc = PatchDocument()..assetDirectory = '/assets';
    final client = FakeClient();
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(body: ToneWorkbench(document: doc, client: client))));
    await tester.tap(find.text('Capture draft').first);
    await tester.tap(find.text('Capture draft').last);
    await tester.pump();
    await tester.tap(find.text('Play A'));
    await tester.pumpAndSettle();
    expect(client.calls.single['type'], 'render_patch_comparison');
    expect(client.calls.single['source'], 'built_in');
    expect((client.calls.single['a'] as Map)['directory'], '/assets');
    expect(find.textContaining('render unavailable'), findsOneWidget);
  });
  testWidgets('compact Patch Lab opens Workbench in its control bay',
      (tester) async {
    tester.view.physicalSize = const Size(480, 600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final doc = PatchDocument();
    await tester.pumpWidget(MaterialApp(
        home: Scaffold(
            body: Center(
      child: SizedBox(
          width: 480,
          height: 210,
          child: PatchEditor(document: doc, client: FakeClient())),
    ))));
    expect(tester.takeException(), isNull);
    expect(find.byKey(const ValueKey('patch-handle-1')), findsOneWidget);
    await tester.tap(find.text('Compare rigs'));
    await tester.pumpAndSettle();
    expect(find.text('TONE WORKBENCH'), findsOneWidget);
    expect(find.text('Tone Workbench'), findsNothing);
    expect(find.byTooltip('Close'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.tap(find.text('Capture draft').first);
    await tester.pump();
    expect(find.text('2 nodes • 48000 Hz'), findsOneWidget);
  });
}
