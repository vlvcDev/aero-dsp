part of 'main.dart';

/// Nearest equal-tempered note to a frequency.
class ChromaticReading {
  const ChromaticReading(this.name, this.octave, this.midi, this.cents);

  /// e.g. 'F#'.
  final String name;
  final int octave;
  final int midi;

  /// -50..50: how far the input is from [name].
  final double cents;

  static ChromaticReading fromHz(double hz, {double a4 = 440}) {
    final exact = 69 + 12 * math.log(hz / a4) / math.ln2;
    final midi = exact.round();
    return ChromaticReading(
        scaleKeyNames[midi % 12], midi ~/ 12 - 1, midi, (exact - midi) * 100);
  }
}

/// Chromatic clean-input tuner: shows whatever note you play and how far
/// off it is, like a pedal tuner. No string or tuning to choose.
class TunerSheet extends StatefulWidget {
  const TunerSheet({required this.meters, super.key});

  final Stream<MeterSnapshot> meters;

  /// Readings below this confidence are treated as no signal.
  static const minimumConfidence = .55;

  /// Within this many cents counts as in tune.
  static const inTuneCents = 3.0;

  @override
  State<TunerSheet> createState() => _TunerSheetState();
}

class _TunerSheetState extends State<TunerSheet> {
  StreamSubscription<MeterSnapshot>? _subscription;
  ChromaticReading? _reading;
  double _cents = 0, _hz = 0;

  /// Meter frames without a confident pitch since the last one.
  int _silentFrames = 99;
  Timer? _fade;
  double _a4 = 440;

  /// The last note stays up (dimmed) this long after the string stops.
  static const _hold = Duration(milliseconds: 1500);

  /// A frame or two of dropout (pick noise, a bend) doesn't dim the display.
  bool get _sounding => _silentFrames < 3;

  @override
  void initState() {
    super.initState();
    _subscription = widget.meters.listen(_receive);
  }

  @override
  void dispose() {
    _subscription?.cancel();
    _fade?.cancel();
    super.dispose();
  }

  void _receive(MeterSnapshot meter) {
    if (!mounted) return;
    if (meter.tunerHz <= 0 ||
        meter.tunerConfidence < TunerSheet.minimumConfidence) {
      // Keep redrawing so the display dims; clear it after the hold.
      _silentFrames++;
      if (_reading != null) {
        setState(() {});
        _fade ??= Timer(_hold, () {
          _fade = null;
          if (mounted) setState(() => _reading = null);
        });
      }
      return;
    }
    _fade?.cancel();
    _fade = null;
    final reading = ChromaticReading.fromHz(meter.tunerHz, a4: _a4);
    setState(() {
      // Smooth the needle while the same note rings; jump on a new note.
      _cents = reading.midi == _reading?.midi && _sounding
          ? _cents * .55 + reading.cents * .45
          : reading.cents;
      _hz = meter.tunerHz;
      _reading = reading;
      _silentFrames = 0;
    });
  }

  void _setA4(double value) => setState(() => _a4 = value.clamp(430, 450));

  @override
  Widget build(BuildContext context) {
    final reading = _reading;
    final live = reading != null && _sounding;
    final inTune = live && _cents.abs() <= TunerSheet.inTuneCents;
    final noteColor = reading == null
        ? const Color(0xff3d6ea8)
        : inTune
            ? LostChrome.lime
            : live
                ? LostChrome.pearl
                : const Color(0xff8fb6dc);
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 6, 16, 10),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(children: [
              const Icon(Icons.tune, color: LostChrome.lime, size: 20),
              const SizedBox(width: 8),
              const Expanded(
                child: Text('CHROMATIC TUNER',
                    overflow: TextOverflow.ellipsis,
                    style:
                        TextStyle(fontSize: 17, fontWeight: FontWeight.w900)),
              ),
              IconButton(
                tooltip: 'Lower reference pitch',
                visualDensity: VisualDensity.compact,
                onPressed: _a4 > 430 ? () => _setA4(_a4 - 1) : null,
                icon: const Icon(Icons.remove, size: 18),
              ),
              GestureDetector(
                onDoubleTap: () => _setA4(440),
                child: Text('A4 ${_a4.round()} Hz',
                    key: const ValueKey('tuner-reference'),
                    style: const TextStyle(
                        color: LostChrome.ice, fontWeight: FontWeight.w800)),
              ),
              IconButton(
                tooltip: 'Raise reference pitch',
                visualDensity: VisualDensity.compact,
                onPressed: _a4 < 450 ? () => _setA4(_a4 + 1) : null,
                icon: const Icon(Icons.add, size: 18),
              ),
            ]),
            Expanded(
              child: Container(
                padding: const EdgeInsets.fromLTRB(10, 8, 10, 8),
                decoration: BoxDecoration(
                  gradient: const LinearGradient(
                    begin: Alignment.topCenter,
                    end: Alignment.bottomCenter,
                    colors: [Color(0xee0f407c), Color(0xee081a43)],
                  ),
                  borderRadius: BorderRadius.circular(14),
                  border: Border.all(
                      color: inTune ? LostChrome.lime : LostChrome.ice,
                      width: inTune ? 2 : 1.3),
                ),
                child: Column(children: [
                  Expanded(
                      child: Center(
                          child: AspectRatio(
                    aspectRatio: 2,
                    child: TweenAnimationBuilder<double>(
                      tween: Tween(end: reading == null ? 0 : _cents),
                      duration: const Duration(milliseconds: 90),
                      builder: (context, cents, _) => CustomPaint(
                        key: const ValueKey('tuner-gauge'),
                        painter: TunerGaugePainter(
                            cents: reading == null ? null : cents,
                            live: live,
                            inTune: inTune),
                        child: Align(
                          alignment: Alignment.bottomCenter,
                          child: FractionallySizedBox(
                            heightFactor: .5,
                            child: FittedBox(
                              child: Column(
                                  mainAxisSize: MainAxisSize.min,
                                  children: [
                                    if (reading != null)
                                      _NeighbourNotes(
                                          midi: reading.midi, live: live),
                                    Row(
                                      mainAxisSize: MainAxisSize.min,
                                      crossAxisAlignment:
                                          CrossAxisAlignment.end,
                                      children: [
                                        Text(reading?.name ?? '–',
                                            key: const ValueKey('tuner-note'),
                                            style: TextStyle(
                                                fontSize: 64,
                                                height: 1,
                                                fontWeight: FontWeight.w900,
                                                color: noteColor)),
                                        if (reading != null)
                                          Padding(
                                            padding: const EdgeInsets.only(
                                                left: 2, bottom: 6),
                                            child: Text('${reading.octave}',
                                                style: TextStyle(
                                                    fontSize: 20,
                                                    fontWeight: FontWeight.w800,
                                                    color: noteColor)),
                                          ),
                                      ],
                                    ),
                                  ]),
                            ),
                          ),
                        ),
                      ),
                    ),
                  ))),
                  const SizedBox(height: 6),
                  _TunerLights(cents: live ? _cents : null, inTune: inTune),
                  const SizedBox(height: 4),
                  Text(
                    reading == null
                        ? 'PLAY ANY STRING'
                        : inTune
                            ? 'IN TUNE  •  ${_hz.toStringAsFixed(1)} Hz'
                            : '${_cents >= 0 ? '+' : ''}${_cents.round()} cents  •  ${_hz.toStringAsFixed(1)} Hz',
                    key: const ValueKey('tuner-status'),
                    style: TextStyle(
                        color: inTune ? LostChrome.lime : LostChrome.ice,
                        fontWeight: FontWeight.w800,
                        letterSpacing: .8),
                  ),
                ]),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// The two notes below (left) and above (right) the one you're on.
class _NeighbourNotes extends StatelessWidget {
  const _NeighbourNotes({required this.midi, required this.live});

  final int midi;
  final bool live;

  @override
  Widget build(BuildContext context) {
    final style = TextStyle(
        fontSize: 15,
        fontWeight: FontWeight.w800,
        letterSpacing: .5,
        color: const Color(0xffa2d5f5).withValues(alpha: live ? .85 : .45));
    String name(int offset) => scaleKeyNames[(midi + offset) % 12];
    return Padding(
      padding: const EdgeInsets.only(bottom: 2),
      child: Row(mainAxisSize: MainAxisSize.min, children: [
        Text('${name(-2)}  ${name(-1)}',
            key: const ValueKey('tuner-below'), style: style),
        const SizedBox(width: 22),
        Text('${name(1)}  ${name(2)}',
            key: const ValueKey('tuner-above'), style: style),
      ]),
    );
  }
}

/// Pedal-tuner style lights: flat arrow, centre, sharp arrow.
class _TunerLights extends StatelessWidget {
  const _TunerLights({required this.cents, required this.inTune});

  final double? cents;
  final bool inTune;

  @override
  Widget build(BuildContext context) {
    const off = Color(0xff1d416e);
    const warn = Color(0xffff5a4e);
    final c = cents;
    Widget light(IconData icon, bool on, Color color, String label) => Icon(
          icon,
          size: 26,
          semanticLabel: label,
          color: on ? color : off,
          shadows: on ? [Shadow(color: color, blurRadius: 10)] : null,
        );
    return Row(mainAxisAlignment: MainAxisAlignment.center, children: [
      light(Icons.play_arrow, c != null && !inTune && c < 0, warn,
          'Flat: tune up'),
      const SizedBox(width: 18),
      light(Icons.circle, inTune, LostChrome.lime, 'In tune'),
      const SizedBox(width: 18),
      Transform.flip(
        flipX: true,
        child: light(Icons.play_arrow, c != null && !inTune && c > 0, warn,
            'Sharp: tune down'),
      ),
    ]);
  }
}

/// Needle gauge: ±50 cents across a 120° arc, green zone in the middle.
class TunerGaugePainter extends CustomPainter {
  const TunerGaugePainter(
      {required this.cents, required this.live, required this.inTune});

  /// Null when nothing has been played yet.
  final double? cents;
  final bool live;
  final bool inTune;

  static const _sweep = math.pi * 2 / 3;

  @override
  void paint(Canvas canvas, Size size) {
    // Pivot below the arc so the note name sits inside it.
    final radius =
        math.min(size.width / 2 / math.sin(_sweep / 2), size.height * .95) *
            .95;
    final pivot = Offset(size.width / 2, radius + 6);
    double angle(double c) =>
        -math.pi / 2 + (c.clamp(-50, 50) / 50) * _sweep / 2;
    Offset at(double c, double r) =>
        pivot + Offset(math.cos(angle(c)), math.sin(angle(c))) * r;

    final arc = Rect.fromCircle(center: pivot, radius: radius);
    canvas.drawArc(
        arc,
        angle(-50),
        _sweep,
        false,
        Paint()
          ..color = const Color(0xff20528c)
          ..style = PaintingStyle.stroke
          ..strokeWidth = 6
          ..strokeCap = StrokeCap.round);
    canvas.drawArc(
        arc,
        angle(-TunerSheet.inTuneCents),
        angle(TunerSheet.inTuneCents) - angle(-TunerSheet.inTuneCents),
        false,
        Paint()
          ..color = LostChrome.lime.withValues(alpha: inTune ? 1 : .55)
          ..style = PaintingStyle.stroke
          ..strokeWidth = 8);

    final tick = Paint()
      ..color = const Color(0xffa2d5f5)
      ..strokeCap = StrokeCap.round;
    for (var c = -50; c <= 50; c += 10) {
      final major = c % 50 == 0 || c == 0;
      tick.strokeWidth = major ? 2.4 : 1.4;
      canvas.drawLine(at(c.toDouble(), radius - (major ? 20 : 12)),
          at(c.toDouble(), radius - 5), tick);
    }
    for (final (c, label) in [(-50.0, '♭'), (50.0, '♯')]) {
      final text = TextPainter(
        text: TextSpan(
            text: label,
            style: const TextStyle(
                color: Color(0xffa2d5f5),
                fontSize: 16,
                fontWeight: FontWeight.w800)),
        textDirection: TextDirection.ltr,
      )..layout();
      text.paint(
          canvas, at(c, radius - 34) - Offset(text.width / 2, text.height / 2));
    }

    final value = cents;
    if (value == null) return;
    final color = inTune
        ? LostChrome.lime
        : live
            ? const Color(0xffff7a6e)
            : const Color(0xff6f8fb5);
    final needle = Paint()
      ..color = color
      ..strokeWidth = 3.5
      ..strokeCap = StrokeCap.round;
    canvas.drawLine(at(value, radius * .55), at(value, radius + 2), needle);
    if (live) {
      canvas.drawCircle(
          at(value, radius + 2),
          5,
          Paint()
            ..color = color
            ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 3));
    }
  }

  @override
  bool shouldRepaint(TunerGaugePainter old) =>
      old.cents != cents || old.live != live || old.inTune != inTune;
}
