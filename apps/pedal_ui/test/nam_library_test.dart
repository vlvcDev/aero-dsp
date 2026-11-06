import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';
import 'widget_test.dart' show PedalAppTestShell, FakeCatalogClient;

void main() {
  test('readable names remove file noise but preserve capture settings', () {
    expect(CatalogModel.readableNamName('Boss_DS-1_Gain-7_591649.nam'),
        'Boss DS 1 Gain 7');
    expect(_model('Mystery', gear: 'amp-cab').category, 'Amps');
    expect(_model('Mystery', gear: 'pedal').category, 'Pedals');
    expect(_model('Gain 7').category, 'Unclassified');
    expect(_model('Marshall JCM-800').category, 'Amps');
    expect(_model('Marshall with Tubescreamer').category, 'Unclassified');
  });

  testWidgets('local picker separates categories and hides online provenance',
      (tester) async {
    tester.view.physicalSize = const Size(480, 320);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final catalog = _LibraryCatalog();
    await tester.pumpWidget(PedalAppTestShell(
        child: ModelLibrarySheet(
            catalog: catalog,
            engineConnected: false,
            selectedPrePath: null,
            selectedAmpPath: null,
            initialSlot: ModelSlot.pre,
            onSelected: (_, __) async {})));
    await tester.pumpAndSettle();
    expect(find.text('Boss DS 1 Gain 7'), findsOneWidget);
    expect(find.text('Marshall JCM 800'), findsNothing);
    expect(find.textContaining('TONE3000'), findsNothing);
    await tester.tap(find.text('distortion'));
    await tester.pumpAndSettle();
    expect(find.text('Boss DS 1 Gain 7'), findsOneWidget);
    await tester.tap(find.text('Untagged'));
    await tester.pumpAndSettle();
    expect(find.text('Boss DS 1 Gain 7'), findsNothing);
    await tester.tap(find.text('Amps (1)'));
    await tester.pumpAndSettle();
    expect(find.text('Marshall JCM 800'), findsOneWidget);
    expect(find.text('clean'), findsOneWidget);
    expect(find.text('distortion'), findsNothing);
    expect(find.text('Boss DS 1 Gain 7'), findsNothing);
    await tester.enterText(find.byType(TextField), 'absent');
    await tester.pumpAndSettle();
    expect(find.text('No matching amps on this device.'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });
}

CatalogModel _model(String name,
        {String? gear, List<String> tags = const []}) =>
    CatalogModel(
        name: name,
        path: '/$name.nam',
        sizeBytes: 1024,
        source: 'tone3000:model:123',
        favorite: false,
        gear: gear,
        tags: tags);

class _LibraryCatalog extends FakeCatalogClient {
  @override
  Future<List<CatalogModel>> listModels() async => [
        _model('Boss_DS-1_Gain-7_591649.nam',
            gear: 'pedal', tags: ['distortion']),
        _model('Marshall_JCM-800_123456.nam', gear: 'amp', tags: ['clean']),
        _model('Unknown capture'),
      ];
}
