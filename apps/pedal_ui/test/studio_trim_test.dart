import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';
import 'widget_test.dart'
    show
        MemoryTrackStore,
        TimelineTestClient,
        PedalAppTestShell,
        FakeCatalogClient;

void main() {
  test('removing the last clip cannot resurrect a legacy take', () {
    const track = TrackLane(
        id: 't',
        name: 'Guitar',
        input: TrackInput.guitar,
        takePath: '/old.wav',
        cleanTakePath: '/old-di.wav',
        durationSeconds: 10,
        clips: [
          TimelineClip(
              id: 'c', takePath: '/old.wav', sourceDurationMilliseconds: 10000)
        ]);
    final empty = track.copyWith(clips: []);
    expect(empty.timelineClips, isEmpty);
    expect(empty.playbackPath, isNull);
    expect(TrackLane.fromJson(empty.toJson()).timelineClips, isEmpty);
  });
  testWidgets(
      're-trim uses newest clip full-source coordinates without accumulating offsets',
      (tester) async {
    tester.view.physicalSize = const Size(1280, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final store = MemoryTrackStore();
    store.projects = [
      store.projects.single.copyWith(countInBars: 0, tracks: [
        const TrackLane(
            id: 'g',
            name: 'Guitar',
            input: TrackInput.guitar,
            takePath: '/new.wav',
            cleanTakePath: '/new-di.wav',
            durationSeconds: 10,
            clips: [
              TimelineClip(
                  id: 'old',
                  takePath: '/old.wav',
                  sourceDurationMilliseconds: 4000),
              TimelineClip(
                  id: 'new',
                  takePath: '/new.wav',
                  cleanTakePath: '/new-di.wav',
                  sourceDurationMilliseconds: 10000,
                  timelineStartMilliseconds: 5000,
                  sourceStartMilliseconds: 2000,
                  sourceEndMilliseconds: 8000),
            ])
      ])
    ];
    final client = _TrimClient();
    await tester.pumpWidget(PedalAppTestShell(
        child: TrackManagerSheet(
            client: client,
            catalog: FakeCatalogClient(),
            toneSnapshot: 'Rig',
            store: store)));
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Trim this take again'));
    await tester.pumpAndSettle();
    var slider = tester.widget<RangeSlider>(find.byType(RangeSlider));
    expect(slider.max, 10);
    expect(slider.values, const RangeValues(2, 8));
    expect(client.waveforms.last, '/new.wav');
    slider.onChanged!(const RangeValues(3, 7));
    await tester.pump();
    await tester.tap(find.text('APPLY CLIP EDIT'));
    await tester.pumpAndSettle();
    await tester.pump(const Duration(milliseconds: 350));
    var clip = store.projects.single.tracks.single.timelineClips.last;
    expect(clip.sourceStartMilliseconds, 3000);
    expect(clip.timelineStartMilliseconds, 6000);
    await tester.tap(find.byTooltip('Trim this take again'));
    await tester.pumpAndSettle();
    slider = tester.widget<RangeSlider>(find.byType(RangeSlider));
    expect(slider.values, const RangeValues(3, 7));
    slider.onChanged!(const RangeValues(1, 9));
    await tester.pump();
    await tester.tap(find.text('APPLY CLIP EDIT'));
    await tester.pumpAndSettle();
    await tester.pump(const Duration(milliseconds: 350));
    clip = store.projects.single.tracks.single.timelineClips.last;
    expect(clip.sourceStartMilliseconds, 1000);
    expect(clip.sourceEndMilliseconds, 9000);
    expect(clip.timelineStartMilliseconds, 4000);
    await tester.tap(find.byTooltip('Reamp clean DI with current rig'));
    await tester.pumpAndSettle();
    clip = store.projects.single.tracks.single.timelineClips.last;
    expect(clip.takePath, '/reamped.wav');
    expect(clip.sourceStartMilliseconds, 1000);
    expect(clip.sourceEndMilliseconds, 9000);
    expect(clip.timelineStartMilliseconds, 4000);
    expect(store.projects.single.tracks.single.timelineClips.first.takePath,
        '/old.wav');
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets(
      'retrospective save freezes once and replaces stale timeline clips',
      (tester) async {
    tester.view.physicalSize = const Size(1280, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final store = MemoryTrackStore();
    store.projects = [
      store.projects.single.copyWith(countInBars: 0, tracks: [
        const TrackLane(
            id: 'g',
            name: 'Guitar',
            input: TrackInput.guitar,
            takePath: '/old.wav',
            durationSeconds: 4,
            clips: [
              TimelineClip(
                  id: 'old',
                  takePath: '/old.wav',
                  sourceDurationMilliseconds: 4000)
            ])
      ])
    ];
    final client = _TrimClient();
    await tester.pumpWidget(PedalAppTestShell(
        child: TrackManagerSheet(
            client: client,
            catalog: FakeCatalogClient(),
            toneSnapshot: 'Rig',
            store: store)));
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Save latest guitar take'));
    await tester.pumpAndSettle();
    expect(client.saves, 1);
    expect(client.waveforms.last, '/snapshot.wav');
    tester
        .widget<RangeSlider>(find.byType(RangeSlider))
        .onChanged!(const RangeValues(2, 6));
    await tester.pump();
    await tester.tap(find.text('APPLY CLIP EDIT'));
    await tester.pumpAndSettle();
    final clip = store.projects.single.tracks.single.timelineClips.single;
    expect(clip.takePath, '/snapshot.wav');
    expect(clip.sourceStartMilliseconds, 2000);
    expect(clip.sourceEndMilliseconds, 6000);
    expect(client.saves, 1);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
  });
}

class _TrimClient extends TimelineTestClient {
  @override
  Future<RiffCapture> reampRiff(
          {required String cleanPath, required String name}) async =>
      RiffCapture(
          id: 'reamped',
          name: name,
          createdAt: DateTime(2026),
          durationSeconds: 10,
          cleanPath: cleanPath,
          processedPath: '/reamped.wav',
          preset: 'Rig');
  final waveforms = <String>[];
  int saves = 0;
  @override
  Future<List<double>> riffWaveform(String path, {int points = 128}) async {
    waveforms.add(path);
    return [.1, .8, .2];
  }

  @override
  Future<RiffBufferStatus> riffBufferStatus() async =>
      const RiffBufferStatus(availableSeconds: 10, capacitySeconds: 180);
  @override
  Future<RiffCapture> saveRiff(String name,
      {double? trimStartSeconds, double? trimEndSeconds}) async {
    saves++;
    return RiffCapture(
        id: 'snapshot',
        name: name,
        createdAt: DateTime(2026),
        durationSeconds: 10,
        cleanPath: '/snapshot-di.wav',
        processedPath: '/snapshot.wav',
        preset: 'Rig');
  }
}
