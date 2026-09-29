import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  testWidgets('shows the disconnected rig shell', (tester) async {
    await tester.pumpWidget(const PedalApp());

    expect(find.text('AERO>>DSP'), findsOneWidget);
    expect(find.text('SIGNAL PATH'), findsOneWidget);
    expect(find.text('NAM'), findsNWidgets(2));
    expect(find.text('GATE'), findsOneWidget);
    expect(find.text('Waiting for engine'), findsOneWidget);
    expect(find.byTooltip('Test audio'), findsOneWidget);
    expect(find.byTooltip('AI Tone Maker'), findsOneWidget);
    expect(find.byTooltip('Riff Vault'), findsOneWidget);
    expect(find.byTooltip('Reconnect engine'), findsOneWidget);
    expect(find.text('INPUT SCOPE'), findsNothing);
    expect(find.text('AERO LAGOON'), findsNothing);
    expect(find.text('T3K'), findsOneWidget);
    expect(find.text('SND'), findsOneWidget);
    expect(find.text('IN'), findsOneWidget);
    expect(find.text('SND'), findsOneWidget);
    expect(find.textContaining('XRUN 0'), findsOneWidget);
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

    expect(find.text('TONE MAKER'), findsOneWidget);
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
    await tester.tap(find.text('PLAN MY TONE'));
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
      'xruns': 2,
      'tuner_hz': 82.41,
      'tuner_confidence': .91,
    });

    expect(meters.inputDb, -12.5);
    expect(meters.outputDb, -3.0);
    expect(meters.clipped, isTrue);
    expect(meters.cpuPercent, 8.25);
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
