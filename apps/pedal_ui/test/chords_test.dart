import 'dart:io';
import 'dart:ui' as ui;
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';
import 'widget_test.dart'
    show PedalAppTestShell, FakeControlClient, FakeCatalogClient;

void main() {
  test('saved progressions persist through a fresh store', () async {
    final dir = await Directory.systemTemp.createTemp('aero-chords-test');
    addTearDown(() => dir.delete(recursive: true));
    final path = '${dir.path}/saved.json';
    await ChordProgressionStore(path: path).save([
      {
        'name': 'Dreamy',
        'steps': [
          {'chord': 'C:maj7', 'shape': 1, 'beats': 3}
        ],
        'bpm': 110
      }
    ]);
    final restored = await ChordProgressionStore(path: path).load();
    expect(restored.single['bpm'], 110);
    expect(restored.single['steps'][0]['shape'], 1);
  });

  testWidgets(
      'chord workspace builds a progression and uses native loop preview',
      (tester) async {
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final catalog = _ChordsCatalog(),
        client = _ChordClient(),
        store = _MemoryStore();
    Widget screen(bool active) => PedalAppTestShell(
        child: ChordWorkspace(
            catalog: catalog, client: client, store: store, active: active));
    await tester.pumpWidget(screen(true));
    await tester.pumpAndSettle();
    expect(find.text('CHORD ATLAS'), findsOneWidget);
    await tester.ensureVisible(find.text('Add chord'));
    await tester.tap(find.text('Add chord'));
    await tester.pump();
    expect(find.text('1. C • 4 beats'), findsOneWidget);
    await tester.ensureVisible(find.text('Save progression'));
    await tester.tap(find.text('Save progression'));
    await tester.pumpAndSettle();
    expect(store.items.single['steps'][0]['chord'], 'C:major');
    await tester.ensureVisible(find.text('Play progression'));
    await tester.tap(find.text('Play progression'));
    await tester.pumpAndSettle();
    expect(client.started!['loop'], true);
    expect(client.started!['record'], false);
    expect(client.started!['tracks'][0]['clips'][0]['path'],
        '/tmp/chord-fixture.wav');
    await tester.pumpWidget(screen(false));
    await tester.pumpAndSettle();
    expect(client.stops, 1);
    expect(tester.takeException(), isNull);
  });

  testWidgets('preview refuses to interrupt recording', (tester) async {
    final client = _ChordClient()..recording = true;
    final catalog = _ChordsCatalog();
    await tester.pumpWidget(PedalAppTestShell(
        child: ChordWorkspace(
            catalog: catalog,
            client: client,
            store: _MemoryStore(),
            active: true)));
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.text('Hear chord'));
    await tester.tap(find.text('Hear chord'));
    await tester.pumpAndSettle();
    expect(find.textContaining('Stop Studio playback or recording'),
        findsOneWidget);
    expect(client.started, isNull);
    expect(catalog.renders, 0);
    expect(client.stops, 0);
  });

  testWidgets('offline ideas work at 480x320 without AI or overflow',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(PedalAppTestShell(
        child: ChordWorkspace(
            catalog: _ChordsCatalog(),
            client: _ChordClient(),
            store: _MemoryStore(),
            active: true)));
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.text('Offline ideas'));
    await tester.tap(find.text('Offline ideas'));
    await tester.pumpAndSettle();
    expect(find.text('Horizon'), findsOneWidget);
    await tester.ensureVisible(find.text('Horizon'));
    await tester.tap(find.text('Horizon'));
    await tester.pumpAndSettle();
    expect(find.text('1. C • 4 beats'), findsOneWidget);
    expect(find.text('2. Dm • 4 beats'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('shape preference is sent and suggested timing is retained',
      (tester) async {
    final catalog = _ChordsCatalog()..variableTiming = true;
    await tester.pumpWidget(PedalAppTestShell(
        child: ChordWorkspace(
            catalog: catalog,
            client: _ChordClient(),
            store: _MemoryStore(),
            active: true)));
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.text('Comfortable transitions'));
    await tester.tap(find.text('Comfortable transitions'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Prefer open shapes').last);
    await tester.pumpAndSettle();
    await tester.ensureVisible(find.text('Offline ideas'));
    await tester.tap(find.text('Offline ideas'));
    await tester.pumpAndSettle();
    expect(catalog.suggestionPayload!['voicing'], 'open');
    await tester.ensureVisible(find.text('Horizon'));
    await tester.tap(find.text('Horizon'));
    await tester.pumpAndSettle();
    expect(find.text('1. C • 2 beats'), findsOneWidget);
    expect(find.text('2. Dm • 6 beats'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
      'selected variation previews, applies and undoes without changing timing',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final catalog = _ChordsCatalog()..variableTiming = true;
    final client = _ChordClient();
    await tester.pumpWidget(PedalAppTestShell(
        child: ChordWorkspace(
            catalog: catalog,
            client: client,
            store: _MemoryStore(),
            active: true)));
    await tester.pumpAndSettle();
    Future<void> tap(String label) async {
      await tester.ensureVisible(find.text(label));
      await tester.tap(find.text(label));
      await tester.pumpAndSettle();
    }

    await tap('Offline ideas');
    await tap('Horizon');
    await tap('Select chords');
    await tap('1. C • 2 beats');
    await tap('Darker');
    expect(catalog.variationPayload!['selected'], [0]);
    await tap('Hear original');
    expect(catalog.previewPayload!['steps'][0]['chord'], 'C:major');
    await tap('Hear variation');
    expect(catalog.previewPayload!['steps'][0]['chord'], 'D:minor');
    await tap('Apply variation');
    expect(client.playing, false);
    expect(find.text('1. Dm • 2 beats'), findsOneWidget);
    expect(find.text('2. Dm • 6 beats'), findsOneWidget);
    await tap('Undo variation');
    expect(find.text('1. C • 2 beats'), findsOneWidget);
    await tap('Darker');
    await tap('Cancel');
    expect(find.text('1. C • 2 beats'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
      'phrase patterns preview and persist as section sketches on small screens',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final catalog = _ChordsCatalog();
    final store = _MemoryStore();
    final client = _ChordClient();
    await tester.pumpWidget(PedalAppTestShell(
        child: ChordWorkspace(
            catalog: catalog, client: client, store: store, active: true)));
    await tester.pumpAndSettle();
    Future<void> tap(String label) async {
      await tester.ensureVisible(find.text(label));
      await tester.tap(find.text(label));
      await tester.pumpAndSettle();
    }

    await tap('Offline ideas');
    await tap('Horizon');
    await tap('Apply to chord 1');
    expect(catalog.phrasePayload!['steps'][0]['pattern'], 'up');
    expect(catalog.phrasePayload!['steps'][1]['pattern'], isNull);
    expect(find.byType(GuitarPhraseTab), findsOneWidget);
    await tap('Play phrase');
    expect(client.started!['tracks'][0]['clips'][0]['path'],
        '/tmp/phrase-fixture.wav');
    await tap('Stop phrase');
    await tap('Save progression');
    expect(store.items.single['steps'][0]['pattern'], 'up');
    expect(store.items.single['steps'][0]['division'], 2);
    expect(store.items.single['section'], 'Custom');
    expect(store.items.single['mode'], 'unspecified');
    expect(tester.takeException(), isNull);
  });

  testWidgets('blue chrome chord workspace visual check', (tester) async {
    if (Platform.environment['AERO_CHORD_SCREENSHOT'] == '1') {
      await tester.runAsync(() async {
        final font = FontLoader('Arial')
          ..addFont(File('/usr/share/fonts/truetype/croscore/Arimo-Regular.ttf')
              .readAsBytes()
              .then(ByteData.sublistView));
        await font.load();
        final icons = FontLoader('MaterialIcons')
          ..addFont(rootBundle.load('fonts/MaterialIcons-Regular.otf'));
        await icons.load();
      });
    }
    tester.view.physicalSize = const Size(1200, 1000);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final key = GlobalKey();
    await tester.pumpWidget(PedalAppTestShell(
        child: RepaintBoundary(
            key: key,
            child: Theme(
                data: ThemeData.dark().copyWith(
                    textTheme:
                        ThemeData.dark().textTheme.apply(fontFamily: 'Arial'),
                    colorScheme: const ColorScheme.dark(
                        primary: LostChrome.ice, secondary: LostChrome.lime)),
                child: DefaultTextStyle(
                    style: const TextStyle(
                        fontFamily: 'Arial', color: LostChrome.pearl),
                    child: ChordWorkspace(
                        catalog: _ChordsCatalog(),
                        client: _ChordClient(),
                        store: _MemoryStore(),
                        active: true))))));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
    if (Platform.environment['AERO_CHORD_SCREENSHOT'] == '1') {
      await tester.runAsync(() async {
        final image = await (key.currentContext!.findRenderObject()
                as RenderRepaintBoundary)
            .toImage();
        final data = await image.toByteData(format: ui.ImageByteFormat.png);
        await File('/tmp/aero-chord-workspace.png')
            .writeAsBytes(data!.buffer.asUint8List());
        image.dispose();
      });
    }
  });
}

class _MemoryStore extends ChordProgressionStore {
  List<Map<String, dynamic>> items = [];
  @override
  Future<List<Map<String, dynamic>>> load() async => items;
  @override
  Future<void> save(List<Map<String, dynamic>> value) async {
    items = value;
  }
}

class _ChordClient extends FakeControlClient {
  bool recording = false, playing = false;
  int stops = 0;
  Map<String, dynamic>? started;
  @override
  Future<Map<String, dynamic>> timelineCommand(
      String command, Map<String, dynamic> payload) async {
    if (command == 'timeline_start') {
      started = payload;
      playing = true;
    }
    if (command == 'timeline_stop') {
      stops++;
      playing = false;
    }
    return {
      'playing': playing,
      'recording': recording,
      'sample_rate': 48000,
      'position_frames': 0
    };
  }
}

class _ChordsCatalog extends FakeCatalogClient {
  int renders = 0;
  bool variableTiming = false;
  Map<String, dynamic>? suggestionPayload;
  Map<String, dynamic>? variationPayload, previewPayload;
  Map<String, dynamic>? phrasePayload;
  @override
  Future<Map<String, dynamic>> chordCommand(
      String command, Map<String, dynamic> payload) async {
    if (command == 'phrase' || command == 'phrase_preview') {
      phrasePayload = payload;
      final phrase = <String, dynamic>{
        'version': 1,
        'ppq': 480,
        'duration_ticks': 3840,
        'outside_key': <String>[],
        'notes': [
          {
            'tick': 0,
            'duration': 192,
            'string': 1,
            'fret': 3,
            'midi': 48,
            'articulation': 'normal'
          }
        ]
      };
      if (command == 'phrase') return phrase;
      return {
        'path': '/tmp/phrase-fixture.wav',
        'duration_ms': 5333,
        'phrase': phrase
      };
    }
    if (command == 'preview') {
      previewPayload = payload;
      renders++;
      return {'path': '/tmp/chord-fixture.wav', 'duration_ms': 2667};
    }
    if (command == 'vary') {
      variationPayload = payload;
      return {
        'steps': [
          for (var i = 0; i < (payload['steps'] as List).length; i++)
            {
              ...payload['steps'][i] as Map<String, dynamic>,
              if ((payload['selected'] as List).contains(i)) 'chord': 'D:minor'
            }
        ]
      };
    }
    if (command == 'suggest') {
      suggestionPayload = payload;
      return {
        'progressions': [
          {
            'name': 'Horizon',
            'description': 'Bright',
            'key': 'C',
            'source': 'Offline collection',
            'steps': [
              {'chord': 'C:major', 'shape': 0, 'beats': variableTiming ? 2 : 4},
              {'chord': 'D:minor', 'shape': 0, 'beats': variableTiming ? 6 : 4}
            ]
          }
        ]
      };
    }
    return {
      'ai_available': false,
      'chords': [
        {
          'id': 'C:major',
          'label': 'C',
          'notes': ['C', 'E', 'G'],
          'shapes': [
            {
              'name': 'Open shape',
              'frets': [-1, 3, 2, 0, 1, 0],
              'fingers': [0, 3, 2, 0, 1, 0]
            }
          ]
        },
        {
          'id': 'D:minor',
          'label': 'Dm',
          'notes': ['D', 'F', 'A'],
          'shapes': [
            {
              'name': 'Open shape',
              'frets': [-1, -1, 0, 2, 3, 1],
              'fingers': [0, 0, 0, 2, 3, 1]
            }
          ]
        },
      ]
    };
  }
}
