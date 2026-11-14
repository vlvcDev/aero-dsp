import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

import 'widget_test.dart' show PedalAppTestShell;

CatalogModel _m(String name, String gear,
        {double? gain, bool cab = false, String kind = 'nam'}) =>
    CatalogModel(
      name: '$name.nam',
      path: '/$name.nam',
      sizeBytes: 1024,
      source: 'local',
      favorite: false,
      gear: gear,
      gainDb: gain,
      includesCab: cab,
      kind: kind,
    );

ToneCard _card(int id, {String? localPath}) => ToneCard(
    id: '$id',
    title: 'Online $id',
    format: 'nam',
    gear: 'amp',
    downloads: 1500,
    creatorName: 'Ada',
    verified: true,
    localPath: localPath);

class _Catalog extends CatalogClient {
  _Catalog({this.connected = true})
      : super(socketPath: '/tmp/pedal-unused-catalog.sock');

  final bool connected;
  final calls = <String>[];
  final touched = <String>[];

  @override
  Future<List<CatalogModel>> listModelsFor(
          {String? kind, String? sort}) async =>
      [
        _m('Amp A', 'amp', gain: 64, cab: true),
        _m('Amp B', 'amp'),
        _m('Fuzz', 'pedal'),
      ];

  @override
  Future<Tone3000Status> tone3000Status() async =>
      Tone3000Status(available: true, connected: connected);

  @override
  Future<ToneResults> tone3000Search({
    String? query,
    String? gear,
    String? sort,
    String? format,
    bool? verified,
    bool? calibrated,
    int page = 1,
    int pageSize = 12,
  }) async {
    calls.add('search q=$query gear=$gear sort=$sort page=$page');
    return ToneResults(
        tones: [_card(page * 10 + 1), _card(page * 10 + 2)],
        page: page,
        totalPages: 3);
  }

  @override
  Future<ToneResults> tone3000Trending({String? gear}) async {
    calls.add('trending gear=$gear');
    return ToneResults(tones: [_card(1)]);
  }

  @override
  Future<ToneLoadResult> tone3000Load(String toneId, {String? modelId}) async {
    calls.add('load $toneId');
    return ToneLoadResult(kind: 'nam', model: _m('dl$toneId', 'amp'));
  }

  @override
  Future<DeviceAuth> beginTone3000DeviceAuth() async => const DeviceAuth(
      userCode: 'ABCD-1234', verificationUri: 'https://t3k.example/device');

  @override
  Future<void> touchModel(String path) async => touched.add(path);
}

Future<void> _open(WidgetTester tester, _Catalog catalog,
    {List<String>? loaded,
    ToneTarget target = ToneTarget.amp,
    Size size = const Size(480, 320)}) async {
  resetToneBrowserMemory();
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(PedalAppTestShell(
    child: ToneBrowser(
      catalog: catalog,
      target: target,
      onSelectModel: (m, slot) async => loaded?.add('${slot.name}:${m.path}'),
      onSelectCabinet: (p) async => loaded?.add('cab:$p'),
    ),
  ));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('library shows gain/CAB badges, filters by gear, fits 480x320',
      (tester) async {
    await _open(tester, _Catalog());
    expect(find.text('Amp A'), findsOneWidget);
    expect(find.text('Fuzz'), findsNothing); // amp target preselects AMP
    expect(find.text('+64 dB'), findsOneWidget);
    expect(find.text('CAB'), findsWidgets);
    await tester.tap(
        find.widgetWithText(ChoiceChip, 'AMP')); // tap active chip to clear
    await tester.pumpAndSettle();
    expect(find.text('Fuzz'), findsOneWidget);
    await tester.enterText(find.byType(TextField), 'amp b');
    await tester.pumpAndSettle();
    expect(find.text('Amp A'), findsNothing);
    expect(find.text('Amp B'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('tapping loads into the slot, stays open, Next steps on',
      (tester) async {
    final loaded = <String>[];
    final catalog = _Catalog();
    await _open(tester, catalog, loaded: loaded);
    await tester.tap(find.text('Amp A'));
    await tester.pumpAndSettle();
    expect(loaded, ['amp:/Amp A.nam']);
    expect(catalog.touched, ['/Amp A.nam']);
    expect(find.text('NOW PLAYING'), findsOneWidget);
    await tester.tap(find.byTooltip('Next tone'));
    await tester.pumpAndSettle();
    expect(loaded.last, 'amp:/Amp B.nam');
    expect(find.text('NOW PLAYING'), findsOneWidget);
    expect(find.byType(ToneBrowser), findsOneWidget);
  });

  testWidgets('TONE3000 search sends gear/sort/page and paginates',
      (tester) async {
    final catalog = _Catalog();
    final loaded = <String>[];
    await _open(tester, catalog, loaded: loaded);
    await tester.tap(find.text('TONE3000'));
    await tester.pumpAndSettle();
    expect(catalog.calls.last, 'search q= gear=amp sort=null page=1');
    await tester.ensureVisible(find.text('DOWNLOADS'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('DOWNLOADS'));
    await tester.pumpAndSettle();
    expect(catalog.calls.last, 'search q= gear=amp sort=downloads-all-time page=1');
    await tester.tap(find.byTooltip('Next page'));
    await tester.pumpAndSettle();
    expect(catalog.calls.last, 'search q= gear=amp sort=downloads-all-time page=2');
    expect(find.text('PAGE 2 / 3'), findsOneWidget);
    await tester.tap(find.text('Online 21'));
    await tester.pumpAndSettle();
    expect(catalog.calls.last, 'load 21');
    expect(loaded, ['amp:/dl21.nam']);
    expect(find.text('IN LIBRARY'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('signed out shows trending and the device code', (tester) async {
    final catalog = _Catalog(connected: false);
    await _open(tester, catalog);
    await tester.tap(find.text('TONE3000'));
    await tester.pumpAndSettle();
    expect(catalog.calls.last, 'trending gear=amp');
    expect(find.text('Online 1'), findsOneWidget);
    await tester.tap(find.text('SIGN IN'));
    await tester.pump(const Duration(milliseconds: 50));
    expect(find.text('ABCD-1234'), findsOneWidget);
    expect(find.text('https://t3k.example/device'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('three-column layout fits 800x600', (tester) async {
    await _open(tester, _Catalog(),
        target: ToneTarget.pre, size: const Size(800, 600));
    expect(find.text('Fuzz'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}
