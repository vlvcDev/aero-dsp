part of 'main.dart';

// Minor pentatonic fretboard trainer (6-string, standard tuning).
//
// Follows two Lukas Kocka lessons:
// - "Music Theory for METAL": the GROUP system. A group is four consecutive
//   scale notes on two strings (two per string). Group n starts on scale
//   degree n (1, b3, 4, 5, b7). Groups chain up the neck 1-2-3-4-5-1 and
//   repeat an octave higher two strings up. Shape n stacks groups
//   n, n-1, n-2 from the low strings up.
// - "2x the SPEED of your Pentatonic": each 2-note-per-string shape becomes
//   3 notes per string by adding the next string's first note on the same
//   string (a doubled pitch, on purpose), plus shred patterns 1-7. Patterns
//   are transcribed from the video's on-screen tab (B minor, 7th fret);
//   pattern 4 is from the narration only (no tab was captured).

const _scaleOpenMidi = [40, 45, 50, 55, 59, 64]; // index 0 = low E
const _pentatonicSteps = [0, 3, 5, 7, 10];
const pentatonicDegreeNames = ['1', '♭3', '4', '5', '♭7'];
const scaleKeyNames = [
  'C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B' //
];

/// Matches `harmonizer::SCALES` in the engine.
const harmonizerScaleNames = [
  'MINOR', 'MAJOR', 'HARM MINOR', 'PHRYGIAN', 'DORIAN', 'PHRYG DOM' //
];

/// Index = interval + 7 (scale steps from the played note).
const harmonizerIntervalNames = [
  'OCT DOWN', '7TH DOWN', '6TH DOWN', '5TH DOWN', '4TH DOWN', '3RD DOWN', //
  '2ND DOWN', 'UNISON', '2ND UP', '3RD UP', '4TH UP', '5TH UP', '6TH UP',
  '7TH UP', 'OCT UP',
];
const scaleMaxFret = 24;
const _scaleStringNames = ['E', 'A', 'D', 'G', 'B', 'e'];
const _groupColors = [
  Color(0xffc9ff68),
  Color(0xff29b9ff),
  Color(0xffffb347),
  Color(0xffff6fae),
  Color(0xffb38cff),
];

class FretNote {
  const FretNote(this.string, this.fret, this.degree);

  /// 0 = low E ... 5 = high e.
  final int string;
  final int fret;

  /// Index into [pentatonicDegreeNames].
  final int degree;
  int get midi => _scaleOpenMidi[string] + fret;
}

/// Minor pentatonic positions for one key on a 24-fret neck.
class PentatonicNeck {
  const PentatonicNeck(this.root);

  /// Pitch class of the key, C = 0.
  final int root;

  /// Fret of the root on the low E string, 0-11.
  int get rootFret => (root - 4) % 12;

  int degreeOf(int midi) => _pentatonicSteps.indexOf((midi - root) % 12);

  String noteName(int midi) => scaleKeyNames[midi % 12];

  String label(FretNote note, {required bool noteNames}) =>
      noteNames ? noteName(note.midi) : pentatonicDegreeNames[note.degree];

  /// Every scale note on the neck.
  List<FretNote> get allNotes => [
        for (var s = 0; s < 6; s++)
          for (var f = 0; f <= scaleMaxFret; f++)
            if (degreeOf(_scaleOpenMidi[s] + f) >= 0)
              FretNote(s, f, degreeOf(_scaleOpenMidi[s] + f)),
      ];

  /// Shape at extended position [k] (0 = shape 1 at the root, 4 = shape 5,
  /// 5 = shape 1 an octave up, -1 = shape 5 an octave down), per string,
  /// low E first. Not range-checked.
  List<List<FretNote>> shapeAt(int k, {bool threeNps = false}) {
    final shape = k % 5;
    final octave = (k - shape) ~/ 5;
    var midi = 40 + rootFret + _pentatonicSteps[shape] + 12 * octave;
    var degree = shape;
    final pitches = <(int, int)>[];
    for (var i = 0; i < 13; i++) {
      pitches.add((midi, degree));
      final next = (degree + 1) % 5;
      midi += (_pentatonicSteps[next] - _pentatonicSteps[degree]) % 12;
      degree = next;
    }
    return [
      for (var s = 0; s < 6; s++)
        [
          for (var i = 0; i < (threeNps ? 3 : 2); i++)
            FretNote(s, pitches[2 * s + i].$1 - _scaleOpenMidi[s],
                pitches[2 * s + i].$2),
        ],
    ];
  }

  /// Shape 1-5 placed on the neck (dropped an octave if it would run past
  /// the last fret).
  List<List<FretNote>> shape(int number, {bool threeNps = false}) {
    final base = shapeAt(number - 1, threeNps: threeNps);
    final frets = base.expand((s) => s).map((n) => n.fret);
    if (frets.reduce(math.max) <= scaleMaxFret) return base;
    return shapeAt(number - 6, threeNps: threeNps);
  }

  /// The four notes on string pair [pair] (0 = E-A, 1 = D-G, 2 = B-e) at
  /// extended position [k]; null when off the neck.
  List<FretNote>? pairAt(int k, int pair) {
    final strings = shapeAt(k);
    final notes = [...strings[2 * pair], ...strings[2 * pair + 1]];
    return notes.every((n) => n.fret >= 0 && n.fret <= scaleMaxFret)
        ? notes
        : null;
  }

  /// Group number (1-5) found at extended position [k] on [pair].
  static int groupAt(int k, int pair) => (k - pair) % 5 + 1;

  /// Every place group [group] appears: all three string pairs, all octaves.
  List<List<FretNote>> groupInstances(int group) => [
        for (var pair = 0; pair < 3; pair++)
          for (var octave = -1; octave <= 2; octave++)
            if (pairAt(group - 1 + pair + 5 * octave, pair) case final notes?)
              notes,
      ];
}

class ShredNote {
  const ShredNote(this.string, this.fret, this.degree,
      {this.stroke, this.technique});
  final int string;
  final int fret;
  final int degree;

  /// 'D' downstroke, 'U' upstroke, null = not picked.
  final String? stroke;

  /// 'H' hammer-on (from nowhere on a run's first note), 'P' pull-off.
  final String? technique;
}

class ShredPattern {
  const ShredPattern(this.number, this.name, this.description,
      {this.fromTab = true, this.acrossShapes = false});
  final int number;
  final String name;
  final String description;

  /// False when built from the narration only (no tab captured).
  final bool fromTab;

  /// Pattern 7 travels through every shape instead of one.
  final bool acrossShapes;
}

const shredPatterns = [
  ShredPattern(1, 'Alternate sixes',
      '1-2-3-1-2-3 on every string, strict alternate picking. Also works as legato.'),
  ShredPattern(2, 'Loop back',
      'Reach to the next string, loop back 3-2-1-2-3, then 1-2-3-1-2-3 on the next string. Strict alternate picking.'),
  ShredPattern(3, 'Legato rolls',
      '1-2-3, hammer-on from nowhere 1-2-3-2-1 on the next string, back 3-2-1-2-3, then 1-2-3. Pick only the marked notes.'),
  ShredPattern(4, 'Economy run',
      'Straight up the 3-notes-per-string shape with economy picking: each string down-up-down, sweeping into the next.',
      fromTab: false),
  ShredPattern(5, 'Economy loops',
      '1-2-3, next string 1-2, loop back for 3, then 1-2-3 on the next string. Economy picking.'),
  ShredPattern(6, 'No doubles (2-1-3)',
      'Every pitch once: 2-3-1-3-1-3 notes per string, up and back down. Economy picking.'),
  ShredPattern(7, 'Sweeps across shapes',
      'Pentatonic arpeggio sweeps stretched over shapes 1-2, then 3, slide into 4, then 5, then the same arpeggio two frets up into shape 1.',
      acrossShapes: true),
];

typedef _ShredStep = (int string, int index, String? stroke, String? technique);

List<_ShredStep> _alternate(List<(int, int)> notes) => [
      for (var i = 0; i < notes.length; i++)
        (notes[i].$1, notes[i].$2, i.isEven ? 'D' : 'U', null),
    ];

/// Pattern 7 as tabbed in B minor: (string, fret, stroke).
const _sweepInB = [
  (5, 12, 'D'), (5, 7, 'U'), (4, 10, 'U'), (3, 9, 'U'), (2, 12, 'U'), //
  (1, 14, 'U'), (1, 12, 'D'), (2, 14, 'D'), (3, 16, 'D'), (4, 12, 'D'),
  (5, 12, 'D'), (5, 17, 'U'), (5, 19, 'D'), (5, 14, 'U'), (4, 17, 'U'),
  (3, 16, 'U'), (2, 19, 'U'), (1, 21, 'U'), (1, 17, 'D'), (2, 19, 'D'),
  (3, 21, 'D'), (4, 17, 'D'), (5, 17, 'D'), (5, 22, 'U'), (5, 24, 'D'),
  (5, 19, 'U'), (4, 19, 'U'), (3, 23, 'U'), (2, 21, 'U'), (1, 19, 'U'),
];

/// Notes of [pattern] over 3-note-per-string [shape] in this key. Null when
/// it does not fit on a 24-fret neck (pattern 7 in C to D#).
List<ShredNote>? buildShredPattern(
    PentatonicNeck neck, int pattern, int shape) {
  if (pattern == 7) {
    final offset = (neck.root - 11) % 12;
    for (final delta in [offset, offset - 12]) {
      final frets = _sweepInB.map((n) => n.$2 + delta);
      if (frets.every((f) => f >= 0 && f <= scaleMaxFret)) {
        return [
          for (final (string, fret, stroke) in _sweepInB)
            ShredNote(string, fret + delta,
                neck.degreeOf(_scaleOpenMidi[string] + fret + delta),
                stroke: stroke),
        ];
      }
    }
    return null;
  }
  final strings = neck.shape(shape, threeNps: true);
  final steps = <_ShredStep>[];
  void legato(int s, List<int> indices, String? stroke) {
    for (var j = 0; j < indices.length; j++) {
      steps.add((
        s,
        indices[j],
        j == 0 ? stroke : null,
        j == 0
            ? (stroke == null ? 'H' : null)
            : (indices[j] > indices[j - 1] ? 'H' : 'P'),
      ));
    }
  }

  switch (pattern) {
    case 1:
      steps.addAll(_alternate([
        for (var s = 0; s < 5; s++)
          for (final i in [0, 1, 2, 0, 1, 2]) (s, i),
        (5, 0),
      ]));
    case 2:
      steps.addAll(_alternate([
        for (var s = 0; s < 4; s++) ...[
          (s + 1, 0),
          for (final i in [2, 1, 0, 1, 2]) (s, i),
          for (final i in [0, 1, 2, 0, 1, 2]) (s + 1, i),
        ],
        (5, 0),
        for (final i in [2, 1, 0, 1, 2]) (4, i),
        (5, 0),
      ]));
    case 3:
      for (var s = 0; s < 4; s++) {
        legato(s, [0, 1, 2], 'D');
        legato(s + 1, [0, 1, 2, 1, 0], null);
        legato(s, [2, 1, 0, 1, 2], 'U');
        legato(s + 1, [0, 1, 2], 'D');
      }
      legato(4, [0, 1, 2], 'D');
      legato(5, [0, 1, 2, 1, 0], null);
    case 4:
      for (var s = 0; s < 6; s++) {
        steps.addAll([(s, 0, 'D', null), (s, 1, 'U', null), (s, 2, 'D', null)]);
      }
    case 5:
      steps.addAll([(0, 0, 'D', null), (0, 1, 'U', null), (0, 2, 'D', null)]);
      for (var s = 0; s < 4; s++) {
        steps.addAll([
          (s + 1, 0, 'D', null),
          (s + 1, 1, 'U', null),
          (s, 2, 'U', null),
          (s + 1, 0, 'D', null),
          (s + 1, 1, 'U', null),
          (s + 1, 2, 'D', null),
        ]);
      }
      steps.addAll([(5, 0, 'D', null), (5, 1, 'U', null), (5, 2, 'D', null)]);
    case 6:
      steps.addAll(const [
        (0, 0, 'U', null), (0, 1, 'D', null), //
        (1, 0, 'D', null), (1, 1, 'U', null), (1, 2, 'D', null),
        (2, 1, 'D', null),
        (3, 0, 'D', null), (3, 1, 'U', null), (3, 2, 'D', null),
        (4, 1, 'D', null),
        (5, 0, 'D', null), (5, 1, 'U', null), (5, 2, 'D', null),
        (5, 1, 'D', null), (5, 0, 'U', null),
        (4, 1, 'U', null),
        (3, 2, 'U', null), (3, 1, 'D', null), (3, 0, 'U', null),
        (2, 1, 'U', null),
        (1, 2, 'U', null), (1, 1, 'D', null), (1, 0, 'U', null),
        (0, 1, 'U', null),
      ]);
    default:
      return null;
  }
  return [
    for (final (s, i, stroke, technique) in steps)
      ShredNote(s, strings[s][i].fret, strings[s][i].degree,
          stroke: stroke, technique: technique),
  ];
}

/// Practice-run lesson: every pattern is tabbed in sixteenth-note triplets.
DailyLesson shredLesson(PentatonicNeck neck, ShredPattern pattern, int shape,
    List<ShredNote> notes, int bpm) {
  final key = '${scaleKeyNames[neck.root]} minor pentatonic';
  return DailyLesson(
    title: pattern.acrossShapes
        ? 'Pattern ${pattern.number} · ${pattern.name}'
        : 'Pattern ${pattern.number} · Shape $shape',
    subtitle: '$key // 16th triplets',
    coaching: pattern.description,
    bpm: bpm,
    lengthBeats: (notes.length / 6).ceilToDouble(),
    events: [
      for (var i = 0; i < notes.length; i++)
        DailyLessonEvent(
          () {
            final midi = _scaleOpenMidi[notes[i].string] + notes[i].fret;
            return '${scaleKeyNames[midi % 12]}${midi ~/ 12 - 1}';
          }(),
          '${6 - notes[i].string} / ${notes[i].fret}',
          beat: i / 6,
        ),
    ],
  );
}

/// CHORDS | SCALES switch for the theory tab.
class TheoryWorkspace extends StatefulWidget {
  const TheoryWorkspace(
      {required this.chords,
      required this.meters,
      required this.onMetronomeChanged,
      required this.active,
      this.onHarmonyKey,
      super.key});
  final Widget Function(bool active) chords;
  final Future<void> Function(int root)? onHarmonyKey;
  final Stream<MeterSnapshot> meters;
  final Future<void> Function(bool enabled, int bpm)? onMetronomeChanged;
  final bool active;

  @override
  State<TheoryWorkspace> createState() => _TheoryWorkspaceState();
}

class _TheoryWorkspaceState extends State<TheoryWorkspace> {
  bool _scales = false;

  @override
  Widget build(BuildContext context) => Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 4, 12, 0),
            child: SegmentedButton<bool>(
              style: const ButtonStyle(
                  visualDensity: VisualDensity.compact,
                  tapTargetSize: MaterialTapTargetSize.shrinkWrap),
              segments: const [
                ButtonSegment(
                    value: false,
                    icon: Icon(Icons.grid_on, size: 14),
                    label: Text('CHORDS')),
                ButtonSegment(
                    value: true,
                    icon: Icon(Icons.linear_scale, size: 14),
                    label: Text('SCALES')),
              ],
              selected: {_scales},
              onSelectionChanged: (v) => setState(() => _scales = v.single),
            ),
          ),
          Expanded(
            child: IndexedStack(
              index: _scales ? 1 : 0,
              children: [
                widget.chords(widget.active && !_scales),
                ScaleWorkspace(
                    meters: widget.meters,
                    onMetronomeChanged: widget.onMetronomeChanged,
                    onHarmonyKey: widget.onHarmonyKey),
              ],
            ),
          ),
        ],
      );
}

enum ScaleView { groups, shapes, navigate, shred }

class ScaleWorkspace extends StatefulWidget {
  const ScaleWorkspace(
      {required this.meters,
      this.onMetronomeChanged,
      this.onHarmonyKey,
      super.key});
  final Stream<MeterSnapshot> meters;

  /// Sets the HARMONY pedal to this minor key.
  final Future<void> Function(int root)? onHarmonyKey;
  final Future<void> Function(bool enabled, int bpm)? onMetronomeChanged;

  @override
  State<ScaleWorkspace> createState() => _ScaleWorkspaceState();
}

class _ScaleWorkspaceState extends State<ScaleWorkspace> {
  PentatonicNeck _neck = const PentatonicNeck(9); // A, as in the lessons
  ScaleView _view = ScaleView.groups;
  bool _noteNames = false, _threeNps = false;
  int _group = 1, _shape = 1, _pattern = 1;

  // Navigator: string pair and extended shape position.
  int _pair = 0, _position = 0;
  (int, int)? _quiz;
  String? _quizPrompt;
  final _random = math.Random();

  void _setKey(int root) => setState(() {
        _neck = PentatonicNeck(root);
        _pair = 0;
        _position = 0;
        _quiz = null;
      });

  String _pairName(int pair) =>
      '${_scaleStringNames[2 * pair]}–${_scaleStringNames[2 * pair + 1]}';

  Map<(int, int), Color> _marks() {
    final marks = <(int, int), Color>{};
    void mark(Iterable<FretNote> notes, Color color) {
      for (final n in notes) {
        marks[(n.string, n.fret)] = color;
      }
    }

    switch (_view) {
      case ScaleView.groups:
        for (final notes in _neck.groupInstances(_group)) {
          mark(notes, _groupColors[_group - 1]);
        }
      case ScaleView.shapes:
        final strings = _neck.shape(_shape, threeNps: _threeNps);
        for (var pair = 0; pair < 3; pair++) {
          final group = (_shape - 1 - pair) % 5;
          mark([...strings[2 * pair], ...strings[2 * pair + 1]],
              _groupColors[group]);
        }
      case ScaleView.navigate:
        if (_neck.pairAt(_position, _pair) case final notes?) {
          mark(notes, LostChrome.lime);
        }
      case ScaleView.shred:
        final notes = buildShredPattern(_neck, _pattern, _shape) ?? const [];
        for (final n in notes) {
          marks[(n.string, n.fret)] = LostChrome.electric;
        }
    }
    return marks;
  }

  bool _canMove(int pair, int position) =>
      pair >= 0 && pair < 3 && _neck.pairAt(position, pair) != null;

  void _move(int pair, int position) {
    if (!_canMove(pair, position)) return;
    setState(() {
      _pair = pair;
      _position = position;
      _quiz = null;
      _quizPrompt = null;
    });
  }

  void _randomMove() {
    final moves = <(int, int, String)>[
      (_pair, _position + 1, 'horizontally → up the neck'),
      (_pair, _position - 1, 'horizontally ← down the neck'),
      (_pair + 1, _position, 'vertically ↑ to the higher strings'),
      (_pair - 1, _position, 'vertically ↓ to the lower strings'),
    ].where((m) => _canMove(m.$1, m.$2)).toList();
    final move = moves[_random.nextInt(moves.length)];
    setState(() {
      _quiz = (move.$1, move.$2);
      _quizPrompt =
          'Move ${move.$3}: play group ${PentatonicNeck.groupAt(move.$2, move.$1)} on ${_pairName(move.$1)}, then REVEAL.';
    });
  }

  Future<void> _practice(List<ShredNote> notes) async {
    final pattern = shredPatterns[_pattern - 1];
    final settings = await showDialog<(int, int)>(
        context: context, builder: (_) => const _PracticeRunSettings());
    if (settings == null || !mounted) return;
    await showAeroControlBay<void>(context,
        title: 'Shred practice',
        icon: Icons.bolt,
        heightFactor: .92,
        child: DailyLessonSheet(
            meters: widget.meters,
            lesson: shredLesson(_neck, pattern, _shape, notes, settings.$1),
            repeatCount: settings.$2,
            customPractice: true,
            onMetronomeChanged: widget.onMetronomeChanged));
  }

  Widget _numberChips(int selected, ValueChanged<int> onSelected,
          {String prefix = '', bool Function(int)? enabled}) =>
      Wrap(spacing: 4, runSpacing: 4, children: [
        for (var n = 1; n <= 5; n++)
          ChoiceChip(
            label: Text('$prefix$n'),
            visualDensity: VisualDensity.compact,
            selected: selected == n,
            onSelected: enabled?.call(n) == false ? null : (_) => onSelected(n),
          ),
      ]);

  Widget _caption(String text) => Padding(
        padding: const EdgeInsets.only(top: 6),
        child: Text(text,
            style: const TextStyle(color: LostChrome.ice, fontSize: 12)),
      );

  List<Widget> _viewControls() {
    switch (_view) {
      case ScaleView.groups:
        final degrees = [
          for (var i = 0; i < 4; i++)
            pentatonicDegreeNames[(_group - 1 + i) % 5]
        ];
        return [
          _numberChips(_group, (n) => setState(() => _group = n),
              prefix: 'GROUP '),
          _caption(
              'Group $_group: lower string ${degrees[0]} ${degrees[1]}, upper string ${degrees[2]} ${degrees[3]}. '
              'It repeats an octave up every two strings (+2 frets, +1 more across the B string). '
              'Next group up the neck shares two of its notes.'),
        ];
      case ScaleView.shapes:
        final groups = [for (var p = 0; p < 3; p++) (_shape - 1 - p) % 5 + 1];
        return [
          Wrap(spacing: 8, runSpacing: 4, children: [
            _numberChips(_shape, (n) => setState(() => _shape = n),
                prefix: 'SHAPE '),
            FilterChip(
              label: const Text('3 NOTES / STRING'),
              visualDensity: VisualDensity.compact,
              selected: _threeNps,
              onSelected: (v) => setState(() => _threeNps = v),
            ),
          ]),
          _caption(
              'Shape $_shape = groups ${groups.join(' · ')} from the low strings up (colours match the groups). '
              '${_threeNps ? 'Three per string: each string adds the next string\'s first note, doubling it on purpose for speed.' : 'Shape n always starts with group n on the low E string.'}'),
        ];
      case ScaleView.navigate:
        final group = PentatonicNeck.groupAt(_position, _pair);
        IconButton arrow(IconData icon, String tip, int pair, int position) =>
            IconButton(
              tooltip: tip,
              visualDensity: VisualDensity.compact,
              onPressed:
                  _canMove(pair, position) ? () => _move(pair, position) : null,
              icon: Icon(icon),
            );
        return [
          Wrap(
              spacing: 2,
              runSpacing: 4,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                arrow(Icons.arrow_back, 'Previous group (down the neck)', _pair,
                    _position - 1),
                arrow(Icons.arrow_forward, 'Next group (up the neck)', _pair,
                    _position + 1),
                arrow(Icons.arrow_downward, 'Lower string pair', _pair - 1,
                    _position),
                arrow(Icons.arrow_upward, 'Higher string pair', _pair + 1,
                    _position),
                const SizedBox(width: 6),
                OutlinedButton(
                    onPressed: _randomMove, child: const Text('RANDOM MOVE')),
                if (_quiz != null)
                  FilledButton(
                      onPressed: () => _move(_quiz!.$1, _quiz!.$2),
                      child: const Text('REVEAL')),
              ]),
          _caption(_quizPrompt ??
              'Group $group on ${_pairName(_pair)}. Horizontal moves step through groups 1→5 along the same strings; '
                  'vertical moves stay in the same shape (group −1 per string pair up).'),
        ];
      case ScaleView.shred:
        final pattern = shredPatterns[_pattern - 1];
        final notes = buildShredPattern(_neck, _pattern, _shape);
        return [
          Wrap(spacing: 4, runSpacing: 4, children: [
            for (final p in shredPatterns)
              ChoiceChip(
                label: Text('P${p.number}'),
                tooltip: p.name,
                visualDensity: VisualDensity.compact,
                selected: _pattern == p.number,
                onSelected: (_) => setState(() => _pattern = p.number),
              ),
          ]),
          if (!pattern.acrossShapes)
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: _numberChips(_shape, (n) => setState(() => _shape = n),
                  prefix: 'SHAPE '),
            ),
          _caption('${pattern.name}: ${pattern.description}'
              '${pattern.fromTab ? '' : ' (From the video narration; no tab captured.)'}'),
          if (notes == null)
            _caption(
                'Doesn\'t fit a 24-fret neck in ${scaleKeyNames[_neck.root]}. Try another key.')
          else ...[
            const SizedBox(height: 6),
            SizedBox(
              height: 150,
              child: SingleChildScrollView(
                scrollDirection: Axis.horizontal,
                child: CustomPaint(
                  key: const ValueKey('shred-tab'),
                  size: Size(ShredTabPainter.widthFor(notes.length), 150),
                  painter: ShredTabPainter(notes),
                ),
              ),
            ),
            Align(
              alignment: Alignment.centerLeft,
              child: FilledButton.icon(
                  onPressed: () => _practice(notes),
                  icon: const Icon(Icons.play_arrow),
                  label: const Text('PRACTICE')),
            ),
          ],
        ];
    }
  }

  @override
  Widget build(BuildContext context) => ListView(
        padding: const EdgeInsets.fromLTRB(12, 6, 12, 12),
        children: [
          Wrap(
            spacing: 8,
            runSpacing: 6,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              DropdownButton<int>(
                value: _neck.root,
                isDense: true,
                dropdownColor: const Color(0xff0b2a5a),
                items: [
                  for (var i = 0; i < 12; i++)
                    DropdownMenuItem(
                        value: i, child: Text('${scaleKeyNames[i]} minor')),
                ],
                onChanged: (v) => _setKey(v!),
              ),
              SegmentedButton<ScaleView>(
                style: const ButtonStyle(
                    visualDensity: VisualDensity.compact,
                    tapTargetSize: MaterialTapTargetSize.shrinkWrap),
                showSelectedIcon: false,
                segments: const [
                  ButtonSegment(value: ScaleView.groups, label: Text('GROUPS')),
                  ButtonSegment(value: ScaleView.shapes, label: Text('SHAPES')),
                  ButtonSegment(
                      value: ScaleView.navigate, label: Text('NAVIGATE')),
                  ButtonSegment(value: ScaleView.shred, label: Text('SHRED')),
                ],
                selected: {_view},
                onSelectionChanged: (v) => setState(() {
                  _view = v.single;
                  _quiz = null;
                  _quizPrompt = null;
                }),
              ),
              FilterChip(
                label: const Text('NOTE NAMES'),
                visualDensity: VisualDensity.compact,
                selected: _noteNames,
                onSelected: (v) => setState(() => _noteNames = v),
              ),
              if (widget.onHarmonyKey case final setHarmony?)
                ActionChip(
                  avatar: const Icon(Icons.queue_music, size: 16),
                  label: const Text('HARMONY IN THIS KEY'),
                  tooltip: 'Set the HARMONY pedal to this minor key',
                  visualDensity: VisualDensity.compact,
                  onPressed: () async {
                    final key = scaleKeyNames[_neck.root];
                    final messenger = ScaffoldMessenger.of(context);
                    try {
                      await setHarmony(_neck.root);
                      messenger.showSnackBar(SnackBar(
                          content: Text('HARMONY pedal set to $key minor')));
                    } catch (error) {
                      messenger.showSnackBar(SnackBar(content: Text('$error')));
                    }
                  },
                ),
            ],
          ),
          const SizedBox(height: 8),
          LayoutBuilder(builder: (context, constraints) {
            final width = math.max(constraints.maxWidth, 25 * 26.0);
            final board = CustomPaint(
              key: const ValueKey('pentatonic-fretboard'),
              size: Size(width, 150),
              painter: PentatonicFretboardPainter(
                  neck: _neck, marks: _marks(), noteNames: _noteNames),
            );
            return width > constraints.maxWidth
                ? SingleChildScrollView(
                    scrollDirection: Axis.horizontal, child: board)
                : board;
          }),
          const SizedBox(height: 4),
          ..._viewControls(),
          const Padding(
            padding: EdgeInsets.only(top: 10),
            child: Text(
                'After Lukas Kocka: "Music Theory for METAL" (groups) and "2x the SPEED of your Pentatonic" (shred patterns).',
                style: TextStyle(color: Color(0xff7fa9c9), fontSize: 10)),
          ),
        ],
      );
}

class PentatonicFretboardPainter extends CustomPainter {
  PentatonicFretboardPainter(
      {required this.neck, required this.marks, required this.noteNames});
  final PentatonicNeck neck;

  /// (string, fret) -> highlight colour. Other scale notes are drawn dim.
  final Map<(int, int), Color> marks;
  final bool noteNames;

  @override
  void paint(Canvas canvas, Size size) {
    const left = 8.0, openWidth = 22.0, right = 6.0, top = 10.0, bottom = 18.0;
    final fretWidth = (size.width - left - openWidth - right) / scaleMaxFret;
    final gap = (size.height - top - bottom) / 5;
    final nut = left + openWidth;
    double x(int fret) =>
        fret == 0 ? left + openWidth / 2 : nut + (fret - .5) * fretWidth;
    double y(int string) => top + (5 - string) * gap;

    final inlay = Paint()..color = LostChrome.ice.withValues(alpha: .14);
    for (final f in [3, 5, 7, 9, 15, 17, 19, 21]) {
      canvas.drawCircle(Offset(x(f), top + 2.5 * gap), 4, inlay);
    }
    for (final f in [12, 24]) {
      canvas.drawCircle(Offset(x(f), top + 1.5 * gap), 4, inlay);
      canvas.drawCircle(Offset(x(f), top + 3.5 * gap), 4, inlay);
    }
    final fretPaint = Paint()
      ..color = LostChrome.ice.withValues(alpha: .35)
      ..strokeWidth = 1;
    for (var f = 1; f <= scaleMaxFret; f++) {
      final fx = nut + f * fretWidth;
      canvas.drawLine(Offset(fx, top), Offset(fx, top + 5 * gap), fretPaint);
    }
    canvas.drawLine(
        Offset(nut, top),
        Offset(nut, top + 5 * gap),
        Paint()
          ..color = LostChrome.pearl
          ..strokeWidth = 3);
    for (var s = 0; s < 6; s++) {
      canvas.drawLine(
          Offset(left, y(s)),
          Offset(size.width - right, y(s)),
          Paint()
            ..color = LostChrome.ice.withValues(alpha: .6)
            ..strokeWidth = 1 + (5 - s) * .25);
    }
    void text(String value, Offset centre, Color color, double fontSize) {
      final painter = TextPainter(
          text: TextSpan(
              text: value,
              style: TextStyle(
                  color: color,
                  fontSize: fontSize,
                  fontWeight: FontWeight.w800)),
          textDirection: TextDirection.ltr)
        ..layout();
      painter.paint(
          canvas, centre - Offset(painter.width / 2, painter.height / 2));
    }

    for (final f in [3, 5, 7, 9, 12, 15, 17, 19, 21, 24]) {
      text('$f', Offset(x(f), size.height - 7),
          LostChrome.ice.withValues(alpha: .6), 9);
    }
    final radius = math.min(fretWidth, gap) * .42;
    for (final note in neck.allNotes) {
      final centre = Offset(x(note.fret), y(note.string));
      final color = marks[(note.string, note.fret)];
      final label = neck.label(note, noteNames: noteNames);
      if (color == null) {
        canvas.drawCircle(centre, radius * .7,
            Paint()..color = LostChrome.ice.withValues(alpha: .12));
        text(label, centre, LostChrome.ice.withValues(alpha: .4), radius * .7);
        continue;
      }
      canvas.drawCircle(centre, radius, Paint()..color = color);
      if (note.degree == 0) {
        canvas.drawCircle(
            centre,
            radius + 1.5,
            Paint()
              ..style = PaintingStyle.stroke
              ..strokeWidth = 2
              ..color = LostChrome.pearl);
      }
      text(label, centre, LostChrome.midnight, radius * .85);
    }
  }

  @override
  bool shouldRepaint(PentatonicFretboardPainter old) =>
      old.neck.root != neck.root ||
      old.noteNames != noteNames ||
      old.marks.length != marks.length ||
      old.marks.entries.any((e) => marks[e.key] != e.value);
}

/// Tab of a shred pattern: picking strokes and hammer/pull marks above, a
/// light tick every beat (six notes).
class ShredTabPainter extends CustomPainter {
  ShredTabPainter(this.notes);
  final List<ShredNote> notes;
  static const step = 24.0, left = 18.0;
  static double widthFor(int count) => left * 2 + count * step;

  @override
  void paint(Canvas canvas, Size size) {
    const lineTop = 44.0;
    final gap = (size.height - lineTop - 10) / 5;
    double y(int string) => lineTop + (5 - string) * gap;
    final line = Paint()
      ..color = LostChrome.ice.withValues(alpha: .45)
      ..strokeWidth = 1;
    for (var s = 0; s < 6; s++) {
      canvas.drawLine(
          Offset(left - 10, y(s)), Offset(size.width - left + 10, y(s)), line);
    }
    void text(String value, Offset centre, Color color, double fontSize) {
      final painter = TextPainter(
          text: TextSpan(
              text: value,
              style: TextStyle(
                  color: color,
                  fontSize: fontSize,
                  fontWeight: FontWeight.w800)),
          textDirection: TextDirection.ltr)
        ..layout();
      painter.paint(
          canvas, centre - Offset(painter.width / 2, painter.height / 2));
    }

    final stroke = Paint()
      ..color = LostChrome.lime
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.6;
    for (var i = 0; i < notes.length; i++) {
      final note = notes[i];
      final cx = left + (i + .5) * step;
      if (i % 6 == 0) {
        canvas.drawLine(
            Offset(cx - step / 2, lineTop - 4),
            Offset(cx - step / 2, y(0) + 4),
            Paint()
              ..color = LostChrome.electric.withValues(alpha: .35)
              ..strokeWidth = 1);
      }
      if (note.stroke == 'D') {
        canvas.drawPath(
            Path()
              ..moveTo(cx - 5, 15)
              ..lineTo(cx - 5, 8)
              ..lineTo(cx + 5, 8)
              ..lineTo(cx + 5, 15),
            stroke);
      } else if (note.stroke == 'U') {
        canvas.drawPath(
            Path()
              ..moveTo(cx - 4, 7)
              ..lineTo(cx, 16)
              ..lineTo(cx + 4, 7),
            stroke);
      }
      if (note.technique != null) {
        text(note.technique!, Offset(cx, 28), LostChrome.electric, 10);
      }
      final centre = Offset(cx, y(note.string));
      canvas.drawRect(Rect.fromCenter(center: centre, width: 20, height: 12),
          Paint()..color = LostChrome.midnight);
      text('${note.fret}', centre,
          note.degree == 0 ? LostChrome.lime : LostChrome.pearl, 11);
    }
  }

  @override
  bool shouldRepaint(ShredTabPainter old) => old.notes != notes;
}
