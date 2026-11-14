import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

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
