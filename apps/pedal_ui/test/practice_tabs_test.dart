import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';
import 'widget_test.dart' show PedalAppTestShell;

void main() {
  test('tab positions, rests and tuning survive tempo changes', () {
    const tab = PracticeTab(
        id: '1',
        name: 'Drop riff',
        tuning: 'Drop D',
        notes: [PracticeTabNote(17, 6, 0), PracticeTabNote(0, 1, 12)]);
    final restored = PracticeTab.fromJson(tab.toJson());
    final slow = restored.lesson(60), fast = restored.lesson(120);
    expect(slow.events.first.note, 'E5');
    expect(slow.events.last.note, 'D2');
    expect(slow.events.last.beat, 4.25);
    expect(slow.timeAt(1), 4250);
    expect(fast.timeAt(1), 2125);
    expect(slow.durationMilliseconds, 16000);
    expect(fast.durationMilliseconds, 8000);
    expect(
        () => PracticeTab.fromJson({
              ...tab.toJson(),
              'notes': [
                {'tick': 0, 'string': 1, 'fret': 0},
                {'tick': 0, 'string': 2, 'fret': 0}
              ]
            }),
        throwsFormatException);
  });

  test('library survives a fresh store without network access', () async {
    final dir = await Directory.systemTemp.createTemp('practice-tabs-test');
    addTearDown(() => dir.delete(recursive: true));
    final path = '${dir.path}/tabs.json';
    await PracticeTabStore(path: path).save([
      const PracticeTab(
          id: 'saved', name: 'My scale', notes: [PracticeTabNote(3, 6, 7)])
    ]);
    final tabs = await PracticeTabStore(path: path).load();
    expect(tabs.single.name, 'My scale');
    expect(tabs.single.notes.single.tick, 3);
    expect(tabs.single.notes.single.fret, 7);
  });

  testWidgets(
      'editor adds a fret, retains off-grid notes and rejects unnamed saves',
      (tester) async {
    tester.view.physicalSize = const Size(900, 850);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(PedalAppTestShell(
        child: PracticeTabEditor(tab: const PracticeTab(id: 'new', name: ''))));
    await tester.tap(find.text('SAVE TAB'));
    await tester.pump();
    expect(find.text('Give your tab a name.'), findsOneWidget);
    await tester.tap(find.text('Sixteenth'));
    await tester.pump();
    await tester.tap(find.bySemanticsLabel('Bar 1, tick 1, string 6'));
    await tester.pumpAndSettle();
    await tester.enterText(find.widgetWithText(TextFormField, '0'), '7');
    await tester.tap(find.text('Set note'));
    await tester.pumpAndSettle();
    expect(find.text('7'), findsOneWidget);
    await tester.tap(find.text('Whole'));
    await tester.pump();
    expect(find.text('7'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
      'custom practice repeats without a second count-in or history write',
      (tester) async {
    tester.view.physicalSize = const Size(900, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final stream = StreamController<MeterSnapshot>.broadcast();
    addTearDown(stream.close);
    final history = _NoHistory();
    var time = DateTime(2026);
    final clicks = <bool>[];
    await tester.pumpWidget(PedalAppTestShell(
        child: DailyLessonSheet(
            meters: stream.stream,
            historyStore: history,
            customPractice: true,
            repeatCount: 2,
            now: () => time,
            onMetronomeChanged: (on, bpm) async {
              clicks.add(on);
            },
            lesson: const PracticeTab(
                    id: 'x',
                    name: 'One bar',
                    bars: 1,
                    notes: [PracticeTabNote(0, 6, 0), PracticeTabNote(8, 5, 0)])
                .lesson(120))));
    await tester.tap(find.text('START EXERCISE 1/1'));
    await tester.pump();
    time = time.add(const Duration(milliseconds: 2010));
    await tester.pump(const Duration(milliseconds: 33));
    stream.add(
        MeterSnapshot.fromJson({'tuner_hz': 82.4069, 'tuner_confidence': .99}));
    await tester.pump();
    expect(find.text('1 HIT • 0 MISSED'), findsOneWidget);
    time = time.add(const Duration(milliseconds: 2000));
    await tester.pump(const Duration(milliseconds: 33));
    expect(find.textContaining('PASS 2/2'), findsOneWidget);
    expect(find.textContaining('COUNT IN'), findsNothing);
    expect(clicks, [true]);
    time = time.add(const Duration(milliseconds: 2000));
    await tester.pump(const Duration(milliseconds: 33));
    expect(find.text('SESSION COMPLETE'), findsOneWidget);
    expect(find.text('1 HIT • 3 MISSED'), findsOneWidget);
    expect(history.writes, 0);
    expect(clicks, [true, false]);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('save errors keep editor and edits available', (tester) async {
    tester.view.physicalSize = const Size(900, 850);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(PedalAppTestShell(
        child: PracticeTabEditor(
            tab: const PracticeTab(id: 'x', name: 'Keep me'),
            onSave: (_) async =>
                throw const FileSystemException('read only'))));
    await tester.tap(find.text('SAVE TAB'));
    await tester.pumpAndSettle();
    expect(find.textContaining('Your edits are still here.'), findsOneWidget);
    expect(find.text('Keep me'), findsOneWidget);
  });

  testWidgets('editor fits the small touchscreen', (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(PedalAppTestShell(
        child: PracticeTabEditor(
            tab: const PracticeTab(id: 'x', name: 'Small screen'))));
    await tester.ensureVisible(find.text('SAVE TAB'));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });

  testWidgets('practice marker stays fixed as the tab moves', (tester) async {
    var time = DateTime(2026);
    await tester.pumpWidget(PedalAppTestShell(
        child: DailyLessonSheet(
            meters: const Stream.empty(),
            customPractice: true,
            repeatCount: 1,
            now: () => time,
            lesson: const PracticeTab(id: 'x', name: 'Long', bars: 64, notes: [
              PracticeTabNote(0, 6, 0),
              PracticeTabNote(24, 1, 12)
            ]).lesson(120))));
    await tester.ensureVisible(find.text('START EXERCISE 1/1'));
    await tester.tap(find.text('START EXERCISE 1/1'));
    await tester.pump();
    final tab = find.byWidgetPredicate((w) =>
        w is CustomPaint &&
        w.painter.runtimeType.toString() == '_TablaturePainter');
    Future<int> markerPixel() async {
      final painter = tester.widget<CustomPaint>(tab).painter!;
      final recorder = ui.PictureRecorder();
      painter.paint(Canvas(recorder), const Size(500, 144));
      final picture = recorder.endRecording();
      final image = await picture.toImage(500, 144);
      final bytes = (await image.toByteData())!;
      final color = bytes.getUint32((12 * 500 + 120) * 4);
      image.dispose();
      picture.dispose();
      return color;
    }

    final before = await tester.runAsync(markerPixel);
    time = time.add(const Duration(seconds: 4));
    await tester.pump(const Duration(milliseconds: 33));
    final after = await tester.runAsync(markerPixel);
    expect(before, after);
    expect(after, isNot(0));
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
  });
}

class _NoHistory extends PracticeHistoryStore {
  int writes = 0;
  @override
  Future<void> add(PracticeSessionRecord record) async {
    writes++;
  }
}
