part of 'main.dart';

/// Saved clip transcription lives outside the real-time audio UI/state.
class GuitarTranscriptionSheet extends StatefulWidget {
  const GuitarTranscriptionSheet({
    super.key,
    required this.catalog,
    required this.clips,
    required this.bpm,
    this.client,
  });

  final CatalogClient catalog;
  final List<TimelineClip> clips;
  final int bpm;
  final ControlClient? client;

  @override
  State<GuitarTranscriptionSheet> createState() =>
      _GuitarTranscriptionSheetState();
}

class _GuitarTranscriptionSheetState extends State<GuitarTranscriptionSheet> {
  static const tunings = {
    'standard': 'Standard · E A D G B E',
    'drop_d': 'Drop D · D A D G B E',
    'half_down': 'Half-step down · E♭ A♭ D♭ G♭ B♭ E♭',
    'whole_down': 'Whole-step down · D G C F A D',
    'drop_c': 'Drop C · C G C F A D',
  };
  late final List<TimelineClip> _clips;
  int _clipIndex = 0;
  String _tuning = 'standard';
  bool _busy = false;
  String? _error;
  Map<String, dynamic>? _status;
  Map<String, dynamic>? _job;

  @override
  void initState() {
    super.initState();
    _clips = widget.clips
        .where((clip) => clip.riffId != null && clip.cleanTakePath != null)
        .toList();
    _checkStatus();
  }

  Future<void> _checkStatus() async {
    try {
      final status = await widget.catalog.transcriptionCommand('status', {});
      if (mounted) {
        setState(() {
          _status = status;
          _error = null;
        });
      }
    } catch (error) {
      if (mounted) setState(() => _error = '$error');
    }
  }

  Future<void> _generate() async {
    final clip = _clips[_clipIndex];
    setState(() {
      _busy = true;
      _error = null;
      _job = null;
    });
    try {
      var job = await widget.catalog.transcriptionCommand('create', {
        'riff_id': clip.riffId,
        'clean_path': clip.cleanTakePath,
        'start_ms': clip.sourceStartMilliseconds,
        'end_ms': clip.sourceEndMilliseconds,
        'tuning': _tuning,
        'bpm': widget.bpm,
      });
      while (mounted && job['status'] == 'running') {
        setState(() => _job = job);
        await Future<void>.delayed(const Duration(seconds: 1));
        if (!mounted) return;
        job =
            await widget.catalog.transcriptionCommand('job', {'id': job['id']});
      }
      if (!mounted) return;
      if (job['status'] == 'failed') throw Exception(job['error']);
      setState(() => _job = job);
    } catch (error) {
      if (mounted) setState(() => _error = '$error');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    if (_job?['status'] == 'complete' && _job?['notes'] is List) {
      return GuitarTabEditor(
        key: ValueKey(_job!['id']),
        initialJob: _job!,
        catalog: widget.catalog,
        client: widget.client,
        clip: _clips[_clipIndex],
        onBack: () => setState(() => _job = null),
      );
    }
    final tab = _job?['tab'] as String?;
    return Padding(
      padding: const EdgeInsets.all(14),
      child: ListView(
        children: [
          Text('CLEAN TAKE → TAB DRAFT',
              style: Theme.of(context).textTheme.titleLarge?.copyWith(
                  color: LostChrome.ice, fontWeight: FontWeight.w900)),
          const SizedBox(height: 8),
          Text(
              'Spotify Basic Pitch · local / offline · ${widget.bpm} BPM reference',
              style: const TextStyle(color: LostChrome.lime)),
          const SizedBox(height: 8),
          const Text(
              'Uses the original clean guitar, not your effects or backing. '
              'Fingerings are suggestions. Chords, bends and fast picking may need corrections.'),
          const SizedBox(height: 12),
          if (_clips.isNotEmpty)
            DropdownButtonFormField<int>(
              initialValue: _clipIndex,
              isExpanded: true,
              decoration: const InputDecoration(labelText: 'Guitar clip'),
              items: [
                for (var i = 0; i < _clips.length; i++)
                  DropdownMenuItem(
                      value: i,
                      child: Text(
                          'Clip ${i + 1} · ${(_clips[i].durationMilliseconds / 1000).toStringAsFixed(2)} s',
                          overflow: TextOverflow.ellipsis))
              ],
              onChanged: _busy
                  ? null
                  : (value) => setState(() {
                        _clipIndex = value!;
                        _job = null;
                      }),
            )
          else
            const Text(
                'Save a guitar take with clean DI before generating tabs.'),
          const SizedBox(height: 12),
          DropdownButtonFormField<String>(
            initialValue: _tuning,
            isExpanded: true,
            decoration: const InputDecoration(
                labelText: 'Recorded tuning · low to high'),
            items: tunings.entries
                .map((entry) => DropdownMenuItem(
                    value: entry.key,
                    child: Text(entry.value, overflow: TextOverflow.ellipsis)))
                .toList(),
            onChanged: _busy
                ? null
                : (value) => setState(() {
                      _tuning = value!;
                      _job = null;
                    }),
          ),
          const SizedBox(height: 12),
          if (_status?['available'] == false) ...[
            SelectableText(_status?['setup'] as String? ??
                'Basic Pitch is not installed.'),
            TextButton(
                onPressed: _checkStatus,
                child: const Text('Check installation again')),
          ],
          if (_error != null) ...[
            Text(_error!, style: const TextStyle(color: Color(0xffffaabd))),
            const SizedBox(height: 8),
          ],
          FilledButton.icon(
            onPressed:
                !_busy && _clips.isNotEmpty && _status?['available'] == true
                    ? _generate
                    : null,
            icon: const Icon(Icons.queue_music_rounded),
            label: Text(_busy
                ? 'Transcribing clean guitar…'
                : 'Generate / open saved tabs'),
          ),
          if (_busy) ...[
            const SizedBox(height: 14),
            const LinearProgressIndicator(),
            const SizedBox(height: 8),
            const Text(
                'The first run can take longer. You may close this window; '
                'the job continues and its result is saved locally.'),
          ],
          if (tab != null) ...[
            const SizedBox(height: 16),
            Container(
              padding: const EdgeInsets.all(12),
              decoration: BoxDecoration(
                gradient: const LinearGradient(
                    colors: [Color(0xff123f66), LostChrome.navy]),
                border: Border.all(color: LostChrome.ice),
                borderRadius: BorderRadius.circular(14),
              ),
              child: SingleChildScrollView(
                scrollDirection: Axis.horizontal,
                child: SelectableText(tab,
                    style: const TextStyle(
                        fontFamily: 'monospace',
                        fontSize: 17,
                        height: 1.5,
                        color: LostChrome.pearl)),
              ),
            ),
            TextButton.icon(
              onPressed: () => Clipboard.setData(ClipboardData(text: tab)),
              icon: const Icon(Icons.copy_rounded),
              label: const Text('Copy tabs'),
            ),
            const Text('Saved locally as a tab draft, note data and MIDI. '
                'Timing is unquantized; columns represent attacks, not equal beats.'),
            SelectableText('${_job?['tab_path']}\n${_job?['midi_path']}',
                style: const TextStyle(color: LostChrome.ice, fontSize: 12)),
          ],
        ],
      ),
    );
  }
}
