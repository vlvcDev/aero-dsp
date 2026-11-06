import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  testWidgets(
      'capture carousel switches offline and retains selection on failure',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final catalog = CaptureTestCatalog();
    String? selected;
    bool fail = false;
    await tester.pumpWidget(PedalAppTestShell(
        child: SlotControlsSheet(
      slot: ModelSlot.amp,
      snapshot: EngineSnapshot.fromJson(
          {'bypassed': false, 'preset': 'Test', 'model': '/a.nam'}),
      onChanged: (_, __) async {},
      onChangeTone: () {},
      catalog: catalog,
      onCaptureSelected: (m) async {
        if (fail) throw Exception('load failed');
        selected = m.path;
      },
    )));
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Next capture'));
    await tester.pumpAndSettle();
    expect(selected, '/b.nam');
    expect(find.textContaining('2 / 2 cached captures'), findsOneWidget);
    fail = true;
    await tester.tap(find.byTooltip('Previous capture'));
    await tester.pumpAndSettle();
    expect(find.textContaining('2 / 2 cached captures'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
      'capture download permits a subset and skips already cached captures',
      (tester) async {
    final catalog = CaptureTestCatalog();
    await tester.pumpWidget(PedalAppTestShell(
        child: Tone3000BrowserSheet(
      catalog: catalog,
      engineConnected: false,
      onSelected: (_, __) async {},
    )));
    await tester.pumpAndSettle();
    expect(find.text('Download 3 captures'), findsOneWidget);
    await tester.tap(find.text('Skip me'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Download 2 captures'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Download'));
    await tester.pumpAndSettle();
    expect(catalog.downloads, ['2']);
    expect(tester.takeException(), isNull);
  });

  testWidgets('tab edits are protected when closing the control bay',
      (tester) async {
    tester.view.physicalSize = const Size(1000, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final catalog = TabEditTestCatalog();
    await tester.pumpWidget(PedalAppTestShell(
        child: Builder(
            builder: (context) => TextButton(
                onPressed: () => showAeroControlBay<void>(context,
                    title: 'Tabs',
                    icon: Icons.music_note,
                    child: GuitarTabEditor(
                        initialJob: catalog.job,
                        catalog: catalog,
                        clip: const TimelineClip(
                            id: 'c',
                            takePath: '/wet.wav',
                            sourceDurationMilliseconds: 2000))),
                child: const Text('Open tabs')))));
    await tester.tap(find.text('Open tabs'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Restore original'));
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Close'));
    await tester.pumpAndSettle();
    expect(find.text('Discard unsaved tab edits?'), findsOneWidget);
    await tester.tap(find.text('Keep editing'));
    await tester.pumpAndSettle();
    expect(find.text('TAB WORKBENCH · 120 BPM'), findsOneWidget);
    await tester.tap(find.byTooltip('Close'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Discard'));
    await tester.pumpAndSettle();
    expect(find.text('TAB WORKBENCH · 120 BPM'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
      'tab editor adds fret corrections, saves, undoes and plays clean offsets',
      (tester) async {
    tester.view.physicalSize = const Size(1000, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final catalog = TabEditTestCatalog();
    final audio = TabPlaybackTestClient();
    await tester.pumpWidget(PedalAppTestShell(
        child: GuitarTabEditor(
      initialJob: catalog.job,
      catalog: catalog,
      client: audio,
      clip: const TimelineClip(
          id: 'c',
          takePath: '/wet.wav',
          cleanTakePath: '/di.wav',
          sourceDurationMilliseconds: 5000,
          sourceStartMilliseconds: 1250,
          sourceEndMilliseconds: 3250),
    )));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Add note'));
    await tester.pumpAndSettle();
    final fret = find.byWidgetPredicate(
        (w) => w is TextField && w.decoration?.labelText == 'Fret (0–24)');
    await tester.enterText(fret, '3');
    await tester.pumpAndSettle();
    final start = find.byWidgetPredicate((w) =>
        w is TextField && w.decoration?.labelText == 'Start (milliseconds)');
    await tester.enterText(start, '127');
    await tester.tap(find.text('Apply note'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Save edits'));
    await tester.pumpAndSettle();
    final saved = catalog.job['notes'] as List;
    expect(saved.length, 2);
    expect(saved.last['midi'], 67);
    expect(saved.last['fret'], 3);
    expect(saved.last['start_seconds'], .127);
    await tester.tap(find.byTooltip('Undo'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Save edits'));
    await tester.pumpAndSettle();
    expect((catalog.job['notes'] as List).length, 1);
    await tester.tap(find.text('Play'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    final tracks = audio.start!['tracks'] as List;
    expect(tracks.first['clips'].first['path'], '/di.wav');
    expect(tracks.first['clips'].first['source_in_ms'], 1250);
    expect(tracks.first['clips'].first['length_ms'], 2000);
    expect(audio.start!['record'], false);
    await tester.tap(find.text('Stop'));
    await tester.pumpAndSettle();
    expect(audio.stops, greaterThan(0));
    await tester.tap(find.text('0.000–2.000 s'));
    await tester.pumpAndSettle();
    await tester.enterText(
        find.byWidgetPredicate(
            (w) => w is TextField && w.decoration?.labelText == 'Start (ms)'),
        '500');
    await tester.enterText(
        find.byWidgetPredicate(
            (w) => w is TextField && w.decoration?.labelText == 'End (ms)'),
        '1500');
    await tester.tap(find.text('Apply'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Loop section'));
    await tester.tap(find.text('Play'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    final loopTracks = audio.start!['tracks'] as List;
    expect(loopTracks.first['clips'].first['source_in_ms'], 1750);
    expect(loopTracks.first['clips'].first['length_ms'], 1000);
    expect(audio.start!['loop'], true);
    await tester.tap(find.text('Stop'));
    await tester.pumpAndSettle();
    await tester.pump(const Duration(seconds: 1));
    expect(tester.takeException(), isNull);
  });

  testWidgets('tab editor scrolls at 480x320', (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final catalog = TabEditTestCatalog();
    await tester.pumpWidget(PedalAppTestShell(
        child: GuitarTabEditor(
      initialJob: catalog.job,
      catalog: catalog,
      clip: const TimelineClip(
          id: 'c',
          takePath: '/wet.wav',
          cleanTakePath: '/di.wav',
          sourceDurationMilliseconds: 2000),
    )));
    await tester.pumpAndSettle();
    await tester.scrollUntilVisible(find.text('Add note'), 100,
        scrollable: find.byType(Scrollable).first);
    expect(tester.takeException(), isNull);
  });

  testWidgets('Basic Pitch tabs fit a touchscreen and preserve source offsets',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final catalog = TranscriptionTestCatalog();
    await tester.pumpWidget(PedalAppTestShell(
        child:
            GuitarTranscriptionSheet(catalog: catalog, bpm: 90, clips: const [
      TimelineClip(
          id: 'clip',
          takePath: '/wet.wav',
          cleanTakePath: '/clean.wav',
          riffId: 'riff-test',
          sourceDurationMilliseconds: 10000,
          sourceStartMilliseconds: 1250,
          sourceEndMilliseconds: 8750),
    ])));
    await tester.pumpAndSettle();
    final generate = find.text('Generate / open saved tabs');
    await tester.scrollUntilVisible(generate, 160,
        scrollable: find.byType(Scrollable).first);
    await tester.tap(generate);
    await tester.pumpAndSettle();
    expect(catalog.request?['start_ms'], 1250);
    expect(catalog.request?['end_ms'], 8750);
    expect(catalog.request?['clean_path'], '/clean.wav');
    expect(catalog.request?['bpm'], 90);
    await tester.scrollUntilVisible(find.text('e|---0---|'), 100,
        scrollable: find.byType(Scrollable).first);
    expect(find.text('e|---0---|'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
      'native studio transport records to a track and persists placement',
      (tester) async {
    tester.view.physicalSize = const Size(1280, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final store = MemoryTrackStore();
    final client = TimelineTestClient();
    await tester.pumpWidget(PedalAppTestShell(
        child: TrackManagerSheet(
            client: client,
            catalog: FakeCatalogClient(),
            toneSnapshot: 'Test rig',
            store: store)));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    await tester.tap(find.byTooltip(
        'Timeline Record/Stop — hold for recording alignment (3 minute max)'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 60));
    expect(client.started?['record'], true);
    expect(client.started?['recording_delay_ms'], 12);
    await tester.tap(find.byTooltip(
        'Timeline Record/Stop — hold for recording alignment (3 minute max)'));
    await tester.pumpAndSettle();
    final clip = store.projects.single.tracks.single.timelineClips.single;
    expect(clip.timelineStartMilliseconds, 1000);
    expect(clip.sourceStartMilliseconds, 12);
    expect(clip.durationMilliseconds, 988);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    await tester.pumpAndSettle();
  });
  testWidgets('audio quality controls scroll on a small touchscreen',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final changes = <String, double>{};
    await tester.pumpWidget(PedalAppTestShell(
        child: AudioQualitySheet(
      snapshot: EngineSnapshot.fromJson({}),
      onChanged: (key, value) async {
        changes[key] = value;
      },
      onMatch: () async => -3.5,
    )));
    await tester.tap(find.text('PORTABLE'));
    await tester.pumpAndSettle();
    expect(changes['quality_profile'], 0);
    await tester.scrollUntilVisible(find.text('MATCH AUDITION LEVEL'), 150,
        scrollable: find.byType(Scrollable).first, continuous: true);
    await tester.tap(find.text('MATCH AUDITION LEVEL'));
    await tester.pumpAndSettle();
    await tester.scrollUntilVisible(find.text('Rig level set to -3.5 dB'), 100,
        scrollable: find.byType(Scrollable).first);
    expect(find.text('Rig level set to -3.5 dB'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
  testWidgets('octaver modes fit a small touchscreen and persist the selection',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final changes = <String, double>{};
    await tester.pumpWidget(PedalAppTestShell(
        child: EffectControlsSheet(
      effect: EffectKind.octaver,
      snapshot: EngineSnapshot.fromJson(
          {'octaver_clean': false, 'octaver_clean_latency_ms': 85.333}),
      onChanged: (key, value) async {
        changes[key] = value;
      },
    )));
    expect(tester.takeException(), isNull);
    await tester.tap(find.text('CLEAN'));
    await tester.pumpAndSettle();
    expect(changes['octaver_clean'], 1);
    expect(find.textContaining('+85.3 ms'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.tap(find.text('ANALOG'));
    await tester.pumpAndSettle();
    expect(changes['octaver_clean'], 0);
    expect(find.textContaining('no added buffering'), findsOneWidget);
  });

  testWidgets('shows the disconnected rig shell', (tester) async {
    await tester.pumpWidget(const PedalApp());

    expect(find.text('AERO>>DSP'), findsOneWidget);
    expect(find.text('SIGNAL PATH'), findsOneWidget);
    expect(find.text('NAM'), findsNWidgets(2));
    expect(find.text('GATE'), findsOneWidget);
    expect(find.byTooltip('Presets: Waiting for engine'), findsOneWidget);
    expect(find.byTooltip('Test audio'), findsOneWidget);
    expect(find.byTooltip('AI Tone Maker'), findsOneWidget);
    expect(find.byTooltip('Riff Vault'), findsOneWidget);
    expect(find.byTooltip('Connecting'), findsOneWidget);
    expect(find.text('INPUT SCOPE'), findsNothing);
    expect(find.text('AERO LAGOON'), findsNothing);
    expect(find.byTooltip('TONE3000'), findsOneWidget);
    expect(find.byTooltip('Output'), findsOneWidget);
    expect(find.text('IN'), findsOneWidget);
    expect(find.text('AUDIO DISABLED'), findsOneWidget);
  });

  testWidgets('rig shell fits the target 480x320 touchscreen', (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(const PedalApp());
    await tester.pump();

    expect(tester.takeException(), isNull);
    expect(find.byTooltip('Test audio'), findsOneWidget);
    expect(find.byTooltip('TONE3000'), findsOneWidget);
    expect(find.text('OUT'), findsOneWidget);
  });

  test('catalog model formats offline metadata for the touch UI', () {
    final model = CatalogModel.fromJson({
      'name': 'Clean Combo',
      'path': '/var/lib/pedal/models/clean.nam',
      'size_bytes': 1572864,
      'source': 'tone3000:model:42',
      'favorite': false,
    });

    expect(model.sourceLabel, 'TONE3000');
    expect(model.sizeLabel, '1.5 MB');
  });

  test('TONE3000 response models parse catalog protocol data', () {
    final status = Tone3000Status.fromJson({
      'available': true,
      'connected': true,
      'auth_pending': false,
      'selected_tone_id': '7',
    });
    final tone = RemoteTone.fromJson({
      'id': '7',
      'name': 'Clean Combo',
      'author': 'Ada',
      'gear': 'amp',
    });
    final model = RemoteModel.fromJson({
      'id': '9',
      'tone_id': '7',
      'name': 'Bright',
    });

    expect(status.connected, isTrue);
    expect(status.selectedToneId, '7');
    expect(tone.author, 'Ada');
    expect(model.toneId, '7');
  });

  test('AI tone recommendation parses a selectable ranked rig', () {
    final recommendation = AiToneRecommendation.fromJson({
      'recommendation_id': 'ai-1',
      'ranking_provider': 'jev',
      'plan': {
        'summary': 'Tight rhythm rig',
        'pedal_query': 'tight overdrive',
        'amp_query': 'high gain amp',
        'controls': {'pedal_drive_db': 8.0},
        'effects': {'eq': true},
      },
      'rigs': [
        {
          'label': 'Focused metal',
          'rationale': 'This one stays tight and clear.',
          'ranking_confidence': .87,
          'ranking_score': .91,
          'pedal': {
            'id': '7',
            'name': 'Drive',
            'author': 'Ada',
            'gear': 'pedal'
          },
          'amp': {'id': '8', 'name': 'Amp', 'author': 'Ada', 'gear': 'amp'},
        },
      ],
    });

    expect(recommendation.id, 'ai-1');
    expect(recommendation.plan.controls['pedal_drive_db'], 8);
    expect(recommendation.rigs.single.pedal?.name, 'Drive');
    expect(recommendation.rigs.single.amp.gear, 'amp');
    expect(recommendation.rankingProvider, 'jev');
    expect(recommendation.rigs.single.confidenceLabel, 'HIGH');
  });

  test('applied AI tone retains a local-only Tone Memory recipe', () {
    final application = AiToneApplication.fromJson({
      'summary': 'Tight rhythm rig',
      'pre_model': {
        'name': 'Boost.nam',
        'path': '/models/boost.nam',
        'size_bytes': 123,
        'source': 'tone3000:ai:pedal-1',
        'favorite': false,
      },
      'amp_model': {
        'name': 'Amp.nam',
        'path': '/models/amp.nam',
        'size_bytes': 456,
        'source': 'tone3000:ai:amp-1',
        'favorite': false,
      },
      'controls': {'amp_mid_db': 2.0},
      'effects': {'eq': true},
      'tone_memory': {
        'query': 'tight metal rhythm',
        'summary': 'Tight rhythm rig',
        'rig_label': 'Focused metal',
        'pre_model': {
          'name': 'Boost.nam',
          'path': '/models/boost.nam',
          'source': 'tone3000:ai:pedal-1',
        },
        'amp_model': {
          'name': 'Amp.nam',
          'path': '/models/amp.nam',
          'source': 'tone3000:ai:amp-1',
        },
        'controls': {'amp_mid_db': 2.0},
        'effects': {'eq': true},
      },
    });

    expect(application.memoryQuery, 'tight metal rhythm');
    expect(application.memoryCandidate['rig_label'], 'Focused metal');
  });

  test('preview response parses the rendered audio file', () {
    final preview = PreviewAudio.fromJson({
      'path': '/tmp/pedal-preview.wav',
      'duration_seconds': 4.25,
    });

    expect(preview.path, '/tmp/pedal-preview.wav');
    expect(preview.durationSeconds, 4.25);
  });

  test('system output parser finds sinks and their active selection', () {
    final outputs = SystemAudioService.parseOutputs('''
Audio
 ├─ Sinks:
 │  *   63. Laptop Speakers [vol: 0.35]
 │     132. Lil Buds [vol: 0.27]
 ├─ Sources:
 │      64. Microphone [vol: 1.00]
''');

    expect(outputs, hasLength(2));
    expect(outputs.first.name, 'Laptop Speakers');
    expect(outputs.first.isDefault, isTrue);
    expect(outputs.last.isBluetooth, isTrue);
  });

  testWidgets('online tone browser fits the target touchscreen',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(
      PedalAppTestShell(
        child: Tone3000BrowserSheet(
          catalog: FakeCatalogClient(),
          engineConnected: false,
          onSelected: (_, __) async {},
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('TONE3000'), findsOneWidget);
    expect(find.text('Clean Combo'), findsOneWidget);
    expect(find.text('Bright'), findsOneWidget);
    expect(find.text('NAM A2'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('online tone browser clearly explains missing configuration',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      PedalAppTestShell(
        child: Tone3000BrowserSheet(
          catalog: UnconfiguredCatalogClient(),
          engineConnected: false,
          onSelected: (_, __) async {},
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('TONE3000'), findsOneWidget);
    expect(find.textContaining('not configured'), findsOneWidget);
    expect(find.textContaining('still work offline'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('AI Tone Maker fits the target touchscreen', (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      PedalAppTestShell(
        child: ToneMakerSheet(
          catalog: FakeAiCatalogClient(),
          engineConnected: false,
          onApply: (_) async {},
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('AQUA SIGNAL CONSOLE'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('Jev-ranked tone cards disclose confidence without auto-applying',
      (tester) async {
    tester.view.physicalSize = const Size(480, 640);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    var applied = false;

    await tester.pumpWidget(
      PedalAppTestShell(
        child: ToneMakerSheet(
          catalog: FakeAiCatalogClient(),
          engineConnected: true,
          onApply: (_) async => applied = true,
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'tight metal rhythm');
    await tester.tap(find.text('SCAN FOR RIGS'));
    await tester.pumpAndSettle();

    expect(find.text('JEV RANKING • CATALOG METADATA, NOT AUDIO ANALYSIS'),
        findsOneWidget);
    expect(find.text('JEV CONFIDENCE: HIGH • 87%'), findsOneWidget);
    expect(applied, isFalse);
    expect(tester.takeException(), isNull);
  });

  testWidgets('guided tuner locks each target string and allows revisiting it',
      (tester) async {
    final meters = StreamController<MeterSnapshot>();
    addTearDown(meters.close);
    await tester.pumpWidget(
      PedalAppTestShell(child: TunerSheet(meters: meters.stream)),
    );

    expect(find.text('GUIDED STRING'), findsOneWidget);
    expect(find.textContaining('Tune E2 first'), findsOneWidget);

    await tester.tap(find.text('2 A2'));
    await tester.pump();
    expect(find.textContaining('Tune A2 first'), findsOneWidget);

    await tester.tap(find.text('1 E2'));
    await tester.pump();
    meters.add(const MeterSnapshot(
      inputDb: -18,
      outputDb: -18,
      clipped: false,
      cpuPercent: 0,
      xruns: 0,
      tunerHz: 82.41,
      tunerConfidence: .9,
    ));
    await tester.pump();
    expect(find.text('IN TUNE — HOLD IT…'), findsOneWidget);

    await tester.pump(const Duration(milliseconds: 1050));
    expect(find.textContaining('Tune A2 first'), findsOneWidget);
  });

  testWidgets('daily drill presents a touch-safe clean-input exercise',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final meters = StreamController<MeterSnapshot>();
    addTearDown(meters.close);

    await tester.pumpWidget(PedalAppTestShell(
      child: DailyLessonSheet(
        meters: meters.stream,
        lesson: DailyLesson.today(DateTime(2026, 1, 1)),
      ),
    ));
    await tester.pump();

    expect(find.text('DAILY DRILL'), findsOneWidget);
    expect(find.text('TABLATURE'), findsOneWidget);
    expect(find.text('START EXERCISE 1/1'), findsOneWidget);
    expect(find.textContaining('Scores clean pitch + timing'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  test('daily lesson events retain their conventional tab coordinates', () {
    const event = DailyLessonEvent('C#3', '5 / 4');

    expect(event.stringNumber, 5);
    expect(event.fret, 4);
  });

  test('daily drill schedules three distinct exercises each day', () {
    final session = DailyLesson.dailySession(DateTime(2026, 1, 1));

    expect(session, hasLength(3));
    expect(session.map((lesson) => lesson.title).toSet(), hasLength(3));
  });

  test('practice history records preserve a completed drill score', () {
    final record = PracticeSessionRecord.fromJson({
      'completed_at_ms': 1767225600000,
      'accuracy_percent': 87,
      'hit_count': 56,
      'note_count': 66,
      'average_timing_ms': 42,
      'exercise_titles': ['E MINOR PENTATONIC'],
    });

    expect(record.accuracyPercent, 87);
    expect(record.toJson()['hit_count'], 56);
    expect(record.exerciseTitles, ['E MINOR PENTATONIC']);
  });

  test('riff metadata parses local clean and processed takes', () {
    final riff = RiffCapture.fromJson({
      'id': 'riff-1',
      'name': 'Bridge idea',
      'created_at_ms': 1767225600000,
      'duration_seconds': 30.0,
      'clean_path': '/riffs/riff-1-clean.wav',
      'processed_path': '/riffs/riff-1-rig.wav',
      'preset': 'Bark',
    });

    expect(riff.name, 'Bridge idea');
    expect(riff.durationSeconds, 30);
    expect(riff.preset, 'Bark');
  });

  test('engine snapshot displays the pedal into amp chain', () {
    final snapshot = EngineSnapshot.fromJson({
      'bypassed': false,
      'pre_bypassed': true,
      'amp_bypassed': false,
      'preset': 'Drive Rig',
      'preset_id': 'preset-2',
      'preset_dirty': true,
      'pre_model': '/models/drive.nam',
      'model': '/models/amp.nam',
      'pedal_drive_db': 8.5,
      'pedal_mix': 0.75,
      'pedal_level_db': -2.0,
      'amp_bass_db': 2.5,
      'amp_mid_db': -1.0,
      'amp_treble_db': 3.0,
      'amp_volume_db': -4.0,
      'looper_bpm': 120,
      'looper_bars': 2,
      'sample_rate': 48000,
      'buffer_frames': 64,
    });

    expect(snapshot.modelName, 'drive.nam → amp.nam');
    expect(snapshot.preBypassed, isTrue);
    expect(snapshot.ampBypassed, isFalse);
    expect(snapshot.pedalMix, 0.75);
    expect(snapshot.ampBassDb, 2.5);
    expect(snapshot.looperBpm, 120);
    expect(snapshot.looperBars, 2);
    expect(snapshot.presetTitle, 'Drive Rig *');
  });

  test('meter snapshot parses safety and health telemetry', () {
    final meters = MeterSnapshot.fromJson({
      'input_db': -12.5,
      'output_db': -3.0,
      'clipped': true,
      'cpu_percent': 8.25,
      'worst_cpu_percent': 125.0,
      'deadline_misses': 1,
      'audio_status': 'recovering',
      'audio_error': 'Device disconnected',
      'xruns': 2,
      'tuner_hz': 82.41,
      'tuner_confidence': .91,
    });

    expect(meters.inputDb, -12.5);
    expect(meters.outputDb, -3.0);
    expect(meters.clipped, isTrue);
    expect(meters.cpuPercent, 8.25);
    expect(meters.worstCpuPercent, 125.0);
    expect(meters.deadlineMisses, 1);
    expect(meters.audioStatus, 'recovering');
    expect(meters.audioError, 'Device disconnected');
    expect(meters.xruns, 2);
    expect(meters.tunerHz, 82.41);
    expect(meters.tunerConfidence, .91);
  });

  test('preset summary describes its saved model chain', () {
    final preset = PresetSummary.fromJson({
      'id': 'preset-1',
      'name': 'Crunch',
      'pre_model': '/models/drive.nam',
      'model': '/models/amp.nam',
    });

    expect(preset.name, 'Crunch');
    expect(preset.modelSummary, 'drive.nam → amp.nam');
  });

  testWidgets('amp controls fit and expose the expected tone stack',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final snapshot = EngineSnapshot.fromJson({
      'bypassed': false,
      'preset': 'Test',
      'pre_model': '/models/drive.nam',
      'model': '/models/amp.nam',
    });

    await tester.pumpWidget(
      PedalAppTestShell(
        child: SlotControlsSheet(
          slot: ModelSlot.amp,
          snapshot: snapshot,
          onChanged: (_, __) async {},
          onChangeTone: () {},
        ),
      ),
    );

    expect(find.text('AMP CONTROLS'), findsOneWidget);
    expect(find.text('BASS'), findsOneWidget);
    expect(find.text('MID'), findsOneWidget);
    expect(find.text('TREBLE'), findsOneWidget);
    expect(find.text('VOLUME'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('preset library fits the target touchscreen', (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final client = FakeControlClient();
    addTearDown(client.dispose);

    await tester.pumpWidget(
      PedalAppTestShell(
        child: PresetLibrarySheet(
          client: client,
          currentPresetId: 'preset-1',
          suggestedName: 'Crunch',
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('PRESETS'), findsOneWidget);
    expect(find.text('Crunch'), findsOneWidget);
    expect(find.text('SAVE CURRENT RIG'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('add-pedal choices fit the target touchscreen', (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      PedalAppTestShell(
        child: AddPedalSheet(
          snapshot: EngineSnapshot.disconnected(),
          onAdd: (_) async {},
        ),
      ),
    );

    expect(find.text('ADD A PEDAL'), findsOneWidget);
    expect(find.text('EQ'), findsOneWidget);
    expect(find.text('CHORUS'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('quantized looper controls fit the target touchscreen',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    await tester.pumpWidget(
      PedalAppTestShell(
        child: LooperControlsSheet(
          mode: 'stopped',
          bpm: 100,
          bars: 4,
          onAction: (_, {bpm, bars}) async {},
          onRemove: () async => false,
        ),
      ),
    );

    expect(find.text('QUANTIZED RECORD'), findsOneWidget);
    expect(find.text('COUNT IN + RECORD'), findsOneWidget);
    expect(find.text('4 B'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}

class FakeControlClient extends ControlClient {
  FakeControlClient() : super(socketPath: '/tmp/pedal-unused-test.sock');

  @override
  Future<List<PresetSummary>> listPresets() async => const [
        PresetSummary(
          id: 'preset-1',
          name: 'Crunch',
          preModel: '/models/drive.nam',
          model: '/models/amp.nam',
        ),
      ];
}

class MemoryTrackStore extends TrackProjectStore {
  List<TrackProject> projects = [
    TrackProject(
        id: 'p',
        name: 'Session',
        createdAt: DateTime(2026),
        recordingDelayMilliseconds: 12,
        tracks: [TrackLane(id: 'g', name: 'Guitar', input: TrackInput.guitar)])
  ];
  @override
  Future<List<TrackProject>> load() async => projects;
  @override
  Future<void> save(List<TrackProject> value) async {
    projects = value;
  }
}

class TimelineTestClient extends FakeControlClient {
  Map<String, Object?>? started;
  bool recording = false;
  @override
  Future<void> setMetronome(bool enabled, int bpm) async {}
  @override
  Future<List<double>> riffWaveform(String path, {int points = 128}) async =>
      [0.1, 0.2];
  @override
  Future<Map<String, dynamic>> timelineCommand(
      String command, Map<String, Object?> payload) async {
    if (command == 'timeline_start') {
      started = payload;
      recording = payload['record'] == true;
    }
    final take = command == 'timeline_stop' && recording;
    if (command == 'timeline_stop') recording = false;
    return {
      'playing': recording,
      'position_frames': 48000,
      'sample_rate': 48000,
      'count_in_frames': 0,
      if (take)
        'take': {
          'processed_path': '/tmp/test-wet.wav',
          'clean_path': '/tmp/test-di.wav',
          'duration_seconds': 1,
          'timeline_start_ms': 1000,
          'source_in_ms': 12,
          'length_ms': 988
        }
    };
  }
}

class TabEditTestCatalog extends CatalogClient {
  Map<String, dynamic> job = {
    'id': 'test-tab',
    'status': 'complete',
    'revision': 0,
    'bpm': 120,
    'duration_seconds': 2.0,
    'tuning': 'standard',
    'tab': 'test tab',
    'tab_path': '/tab.txt',
    'midi_path': '/tab.mid',
    'notes': [
      <String, dynamic>{
        'start_seconds': 0.0,
        'end_seconds': .1,
        'midi': 64,
        'string': 1,
        'fret': 0,
        'strength': .8,
        'bends': <int>[]
      }
    ],
  };
  @override
  Future<Map<String, dynamic>> transcriptionCommand(
      String command, Map<String, dynamic> payload) async {
    job = {
      ...job,
      'revision': (job['revision'] as int) + 1,
      'notes': payload['notes']
    };
    return job;
  }
}

class TabPlaybackTestClient extends ControlClient {
  Map<String, dynamic>? start;
  int stops = 0;
  @override
  Future<Map<String, dynamic>> timelineCommand(
      String command, Map<String, dynamic> payload) async {
    if (command == 'timeline_start') start = payload;
    if (command == 'timeline_stop') stops++;
    return {
      'playing': command != 'timeline_stop',
      'recording': false,
      'position_frames': 4800,
      'sample_rate': 48000
    };
  }
}

class CaptureTestCatalog extends FakeCatalogClient {
  final downloads = <String>[];
  @override
  Future<List<CatalogModel>> listModels() async => const [
        CatalogModel(
            name: 'A',
            path: '/a.nam',
            sizeBytes: 1,
            source: 'tone3000:model:1',
            favorite: false,
            captureGroup: 'tone3000:7',
            captureId: '1'),
        CatalogModel(
            name: 'B',
            path: '/b.nam',
            sizeBytes: 1,
            source: 'tone3000:model:8',
            favorite: false,
            captureGroup: 'tone3000:7',
            captureId: '8'),
      ];
  @override
  Future<SelectedTone> selectedTone3000() async => const SelectedTone(
        tone: RemoteTone(id: '7', name: 'Head', author: 'Tester', gear: 'amp'),
        models: [
          RemoteModel(id: '1', toneId: '7', name: 'Cached'),
          RemoteModel(id: '2', toneId: '7', name: 'New capture'),
          RemoteModel(id: '3', toneId: '7', name: 'Skip me')
        ],
      );
  @override
  Future<CatalogModel> downloadTone3000Model(
      String toneId, String modelId) async {
    downloads.add(modelId);
    return CatalogModel(
        name: 'New capture',
        path: '/new.nam',
        sizeBytes: 1,
        source: 'tone3000:model:$modelId',
        favorite: false,
        captureGroup: 'tone3000:$toneId',
        captureId: modelId);
  }
}

class TranscriptionTestCatalog extends CatalogClient {
  Map<String, dynamic>? request;
  @override
  Future<Map<String, dynamic>> transcriptionCommand(
      String command, Map<String, dynamic> payload) async {
    if (command == 'status') return {'available': true};
    request = payload;
    return {
      'status': 'complete',
      'tab': 'e|---0---|',
      'tab_path': '/tabs.txt',
      'midi_path': '/notes.mid'
    };
  }
}

class FakeCatalogClient extends CatalogClient {
  FakeCatalogClient() : super(socketPath: '/tmp/pedal-unused-catalog.sock');

  @override
  Future<Tone3000Status> tone3000Status() async => const Tone3000Status(
      available: true, connected: true, selectedToneId: '7');

  @override
  Future<SelectedTone> selectedTone3000() async => const SelectedTone(
        tone: RemoteTone(
            id: '7', name: 'Clean Combo', author: 'Ada', gear: 'amp'),
        models: [RemoteModel(id: '9', toneId: '7', name: 'Bright')],
      );
}

class UnconfiguredCatalogClient extends CatalogClient {
  UnconfiguredCatalogClient()
      : super(socketPath: '/tmp/pedal-unused-catalog.sock');

  @override
  Future<Tone3000Status> tone3000Status() async =>
      const Tone3000Status(available: false, connected: false);
}

class FakeAiCatalogClient extends CatalogClient {
  FakeAiCatalogClient() : super(socketPath: '/tmp/pedal-unused-catalog.sock');

  @override
  Future<ToneMakerStatus> toneMakerStatus() async =>
      const ToneMakerStatus(available: true, tone3000Connected: true);

  @override
  Future<AiToneRecommendation> requestAiTone(String prompt) async =>
      AiToneRecommendation.fromJson({
        'recommendation_id': 'jev-test',
        'ranking_provider': 'jev',
        'plan': {
          'summary': 'Tight high-gain rhythm with a focused finish.',
          'pedal_query': 'tight overdrive',
          'amp_query': 'high gain amp',
          'controls': {
            'pedal_drive_db': 8.0,
            'pedal_mix': 1.0,
            'pedal_level_db': 0.0,
          },
          'effects': {
            'eq': true,
            'chorus': false,
            'delay': false,
            'reverb': false
          },
        },
        'rigs': [
          {
            'label': 'Jev pick 1',
            'rationale': 'Jev rates this as a strong metadata match.',
            'ranking_confidence': .87,
            'ranking_score': .75,
            'pedal': {
              'id': 'pedal-1',
              'name': 'Tight Drive',
              'author': 'Ada',
              'gear': 'pedal',
            },
            'amp': {
              'id': 'amp-1',
              'name': 'High Gain',
              'author': 'Ada',
              'gear': 'amp',
            },
          },
        ],
      });
}

class PedalAppTestShell extends StatelessWidget {
  const PedalAppTestShell({required this.child, super.key});

  final Widget child;

  @override
  Widget build(BuildContext context) => MaterialApp(
        theme: ThemeData.dark(useMaterial3: true),
        home: Scaffold(body: child),
      );
}
