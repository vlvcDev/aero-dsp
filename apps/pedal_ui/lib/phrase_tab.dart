part of 'main.dart';

const phrasePatterns = {
  'block': 'Held chord',
  'up': 'Arpeggio · ascending',
  'down': 'Arpeggio · descending',
  'updown': 'Arpeggio · up/down',
  'skip': 'Arpeggio · string skipping',
  'pulse': 'Rhythm · steady hits',
  'offbeat': 'Rhythm · offbeats',
  'chug': 'Rhythm · muted bass',
};
const phraseModes = ['unspecified', 'major', 'minor', 'dorian', 'mixolydian'];
const phraseSections = [
  'Custom',
  'Intro',
  'Verse',
  'Chorus',
  'Bridge',
  'Outro'
];

/// Exact generated positions and durations, not decorative waveform data.
class GuitarPhraseTab extends StatelessWidget {
  const GuitarPhraseTab(this.phrase, {super.key});
  final Map<String, dynamic> phrase;
  @override
  Widget build(BuildContext context) {
    final beats = (phrase['duration_ticks'] as num) / (phrase['ppq'] as num);
    final description = (phrase['notes'] as List)
        .map((n) =>
            'beat ${(n['tick'] as num) / (phrase['ppq'] as num) + 1}, string ${6 - (n['string'] as int)}, fret ${n['fret']}')
        .join('; ');
    return Semantics(
        label: 'Guitar phrase tablature: $description',
        child: SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: SizedBox(
              width: math.max(360, beats * 96 + 60),
              height: 196,
              child: CustomPaint(painter: _PhraseTabPainter(phrase)),
            )));
  }
}

class _PhraseTabPainter extends CustomPainter {
  _PhraseTabPainter(this.phrase);
  final Map<String, dynamic> phrase;
  void label(Canvas canvas, String text, Offset at, Color color,
      {double size = 13}) {
    final p = TextPainter(
        text: TextSpan(
            text: text,
            style: TextStyle(
                color: color, fontSize: size, fontWeight: FontWeight.bold)),
        textDirection: TextDirection.ltr)
      ..layout();
    p.paint(canvas, at);
  }

  @override
  void paint(Canvas canvas, Size size) {
    final ppq = (phrase['ppq'] as num).toDouble();
    final total = (phrase['duration_ticks'] as num).toDouble();
    final scale = (size.width - 60) / total;
    const names = ['E', 'A', 'D', 'G', 'B', 'e'];
    for (var s = 0; s < 6; s++) {
      final y = 40.0 + (5 - s) * 25;
      canvas.drawLine(Offset(30, y), Offset(size.width, y),
          Paint()..color = LostChrome.ice.withValues(alpha: .45));
      label(canvas, names[s], Offset(7, y - 8), LostChrome.ice);
    }
    for (var tick = 0.0; tick < total; tick += ppq) {
      final x = 40 + tick * scale;
      canvas.drawLine(Offset(x, 25), Offset(x, 175),
          Paint()..color = LostChrome.ice.withValues(alpha: .15));
      label(
          canvas, '${(tick / ppq).round() + 1}', Offset(x, 4), LostChrome.ice);
    }
    for (final n in phrase['notes'] as List) {
      final x = 40 + (n['tick'] as num) * scale;
      final y = 40.0 + (5 - (n['string'] as int)) * 25;
      final width = math.max(3.0, (n['duration'] as num) * scale - 2);
      canvas.drawRRect(
          RRect.fromRectAndRadius(
              Rect.fromLTWH(x, y - 9, width, 18), const Radius.circular(4)),
          Paint()
            ..color = (n['articulation'] == 'muted'
                    ? LostChrome.lime
                    : LostChrome.electric)
                .withValues(alpha: .4));
      label(canvas, '${n['fret']}', Offset(x + 2, y - 8), LostChrome.pearl);
    }
  }

  @override
  bool shouldRepaint(covariant _PhraseTabPainter oldDelegate) =>
      oldDelegate.phrase != phrase;
}
