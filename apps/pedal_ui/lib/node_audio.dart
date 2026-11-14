part of 'main.dart';

// Owns the revision for this editor even while the audio panel is closed.
class PatchLiveController extends ChangeNotifier {
  PatchLiveController(this.document, this.client) {
    document.addListener(changed);
    timer = Timer.periodic(const Duration(seconds: 1), (_) => poll());
  }
  final PatchDocument document;
  final ControlClient client;
  Timer? timer, debounce;
  String? revision, pendingRevision, baseline, directory;
  bool inFlight = false, polling = false, disposed = false;
  DateTime? pendingSince;
  int generation = 0;
  String? error;
  String get draft => document.encode();
  bool get dirty =>
      baseline != null &&
      (draft != baseline || directory != document.assetDirectory);
  String get label => error != null
      ? 'Live update failed: $error'
      : pendingRevision != null
          ? 'Activating rig…'
          : revision == null
              ? 'Unapplied changes'
              : !dirty
                  ? 'Tuning live'
                  : compatible()
                      ? 'Tuning live'
                      : 'Reapply rig to hear structural changes';
  static const fields = <String, Set<String>>{
    'gain': {'db'},
    'fuzz': {'drive_db', 'bias', 'tone', 'mix', 'output_db'},
    'overdrive': {'drive_db', 'tone', 'mix', 'output_db'},
    'edge_amp': {'drive_db', 'tone', 'touch', 'master_db'},
    'chorus': {'rate_hz', 'depth', 'mix'},
    'flanger': {'rate_hz', 'depth', 'feedback', 'mix'},
    'tremolo': {'rate_hz', 'depth'},
    'octaver': {'mix'},
    'reverb': {'decay_seconds'},
    'envelope_filter': {
      'sensitivity',
      'base_hz',
      'resonance',
      'decay_ms',
      'mix'
    },
    'ring_mod': {'frequency_hz', 'mix'},
    'slicer': {'bpm', 'depth'},
    'bitcrusher': {'rate_hz', 'mix'},
  };
  bool compatible() {
    if (baseline == null || directory != document.assetDirectory) return false;
    bool same(dynamic a, dynamic b, [String? kind]) {
      if (a is Map && b is Map) {
        if (a.length != b.length || a.keys.any((k) => !b.containsKey(k))) {
          return false;
        }
        final type = a['type'] is String ? a['type'] as String : kind;
        for (final key in a.keys) {
          if (fields[type]?.contains(key) == true &&
              a[key] is num &&
              b[key] is num) {
            continue;
          }
          if (!same(a[key], b[key], type)) return false;
        }
        return true;
      }
      if (a is List && b is List) {
        return a.length == b.length &&
            List.generate(a.length, (i) => i)
                .every((i) => same(a[i], b[i], kind));
      }
      return a == b;
    }

    return same(jsonDecode(baseline!), document.data);
  }

  void changed() {
    if (disposed) return;
    notifyListeners();
    if (revision != null &&
        dirty &&
        compatible() &&
        !(debounce?.isActive ?? false)) {
      debounce = Timer(const Duration(milliseconds: 34), () {
        debounce = null;
        flush();
      });
    }
  }

  void beginApply() {
    generation++;
    revision = null;
    pendingRevision = null;
    error = null;
    changed();
  }

  void applied(String rev, String snapshot, String folder) {
    if (disposed) return;
    generation++;
    revision = null;
    pendingRevision = rev;
    pendingSince = DateTime.now();
    baseline = snapshot;
    directory = folder;
    notifyListeners();
    unawaited(poll());
  }

  Future<void> poll() async {
    if (polling || disposed) return;
    polling = true;
    final requestGeneration = generation;
    try {
      final status = await client.patchRequest('patch_status');
      if (disposed || generation != requestGeneration) return;
      final current = status['revision'];
      if (pendingRevision != null) {
        if (status['active'] == true &&
            status['switching'] != true &&
            status['faulted'] != true &&
            current == pendingRevision) {
          revision = pendingRevision;
          pendingRevision = null;
          changed();
        } else if (status['switching'] != true &&
            pendingSince != null &&
            DateTime.now().difference(pendingSince!) >
                const Duration(seconds: 3)) {
          pendingRevision = null;
          revision = null;
          notifyListeners();
        }
      } else if (revision != null &&
          (status['available'] != true ||
              status['faulted'] == true ||
              status['active'] != true ||
              current != revision)) {
        revision = null;
        notifyListeners();
      }
    } catch (_) {
      if (!disposed && generation == requestGeneration) {
        revision = null;
        pendingRevision = null;
        notifyListeners();
      }
    } finally {
      polling = false;
    }
  }

  Future<void> flush() async {
    if (disposed || inFlight || revision == null || !dirty || !compatible()) {
      return;
    }
    final rev = revision!, sent = draft, folder = document.assetDirectory;
    final requestGeneration = generation;
    inFlight = true;
    try {
      final response = await client.patchRequest('update_patch',
          {'revision': rev, 'document': jsonDecode(sent), 'directory': folder});
      if (disposed || generation != requestGeneration) return;
      if (revision == rev &&
          response['updated'] == true &&
          response['revision'] == rev) {
        baseline = sent;
        directory = folder;
      } else {
        revision = null;
        error = 'The engine did not accept this revision. Apply the rig again.';
      }
    } catch (e) {
      if (!disposed && generation == requestGeneration) {
        revision = null;
        error = e.toString();
      }
    } finally {
      inFlight = false;
      if (!disposed) changed();
    }
  }

  @override
  void dispose() {
    disposed = true;
    timer?.cancel();
    debounce?.cancel();
    document.removeListener(changed);
    super.dispose();
  }
}

class PatchAudioControls extends StatefulWidget {
  const PatchAudioControls(
      {super.key, required this.document, required this.client, this.live});
  final PatchDocument document;
  final ControlClient client;
  final PatchLiveController? live;
  @override
  State<PatchAudioControls> createState() => _PatchAudioControlsState();
}

class _PatchAudioControlsState extends State<PatchAudioControls> {
  late final PatchLiveController live =
      widget.live ?? PatchLiveController(widget.document, widget.client);
  Timer? _timer;
  bool _polling = false,
      _busy = false,
      _available = false,
      _active = false,
      _switching = false;
  String _status = 'Checking audio engine…';
  String? _message;
  Process? _player;
  int? _rate;
  @override
  void initState() {
    super.initState();
    unawaited(refresh());
    _timer = Timer.periodic(const Duration(seconds: 1), (_) => refresh());
  }

  @override
  void dispose() {
    _timer?.cancel();
    if (widget.live == null) live.dispose();
    _player?.kill();
    super.dispose();
  }

  Future<void> refresh() async {
    if (_polling) return;
    _polling = true;
    try {
      final status = await widget.client.patchRequest('patch_status');
      if (!mounted) return;
      setState(() {
        _available = status['available'] == true;
        _active = status['active'] == true;
        _switching = status['switching'] == true;
        _rate = status['sample_rate'] as int?;
        _status = status['faulted'] == true
            ? 'Patch muted after a processing fault — reapply or return to pedalboard'
            : _switching
                ? 'Switching…'
                : _active
                    ? status['bypassed'] == true
                        ? 'Patch selected • global bypass is ON (main power button)'
                        : _rate != null &&
                                _rate! > 0 &&
                                status['latency_samples'] is num
                            ? 'PATCH LIVE • $_rate Hz • ${((status['latency_samples'] as num) * 1000 / _rate!).toStringAsFixed(1)} ms added'
                            : 'PATCH LIVE • latency unavailable'
                    : _available
                        ? 'Pedalboard live • ${_rate ?? 0} Hz'
                        : 'Live audio unavailable • clip preview still available';
      });
    } catch (_) {
      if (mounted) {
        setState(() {
          _available = false;
          _status = 'Audio engine unavailable';
        });
      }
    } finally {
      _polling = false;
    }
  }

  Future<void> run(String command) async {
    _player?.kill();
    setState(() {
      _busy = true;
      _message = null;
      _player = null;
    });
    // Snapshot the draft before awaiting; later edits never change an in-flight request.
    final payload = <String, Object?>{
      'document': jsonDecode(widget.document.encode()),
      'directory': widget.document.assetDirectory,
      if (widget.document.previewPath.trim().isNotEmpty)
        'path': widget.document.previewPath.trim(),
    };
    if (command == 'apply_patch' || command == 'deactivate_patch') {
      live.beginApply();
    }
    final requestGeneration = live.generation;
    try {
      final response = await widget.client.patchRequest(command, payload);
      // Activation belongs to the editor controller, which outlives this panel.
      // Dismissing the panel while models load must not lose live-control ownership.
      if (!live.disposed &&
          live.generation == requestGeneration &&
          command == 'apply_patch' &&
          response['revision'] is String) {
        live.applied(
            response['revision'] as String,
            const JsonEncoder.withIndent('  ').convert(payload['document']),
            payload['directory'] as String);
      }
      if (!mounted) return;
      if (command == 'render_patch') {
        final player =
            await _startLocalAudioPlayback(response['path'] as String);
        if (!mounted) {
          player.kill();
          return;
        }
        setState(() {
          _player = player;
          _busy = false;
          _message =
              'Playing up to 30 seconds + 3-second tail${(response['attenuation_db'] as num? ?? 0) < 0 ? ' • level reduced to prevent clipping' : ''}';
        });
        unawaited(player.stdout.drain());
        unawaited(player.stderr.drain());
        unawaited(player.exitCode.then((exit) {
          if (mounted && identical(_player, player)) {
            setState(() {
              _player = null;
              if (exit != 0) _message = 'Playback stopped or failed ($exit)';
            });
          }
        }));
      } else {
        setState(() => _message = command == 'apply_patch'
            ? 'Patch queued. Check the live status before playing.'
            : 'Returning to pedalboard…');
        await refresh();
      }
    } catch (error) {
      if (!live.disposed &&
          live.generation == requestGeneration &&
          (command == 'apply_patch' || command == 'deactivate_patch')) {
        live.error = error.toString();
        live.changed();
      }
      if (mounted) setState(() => _message = '$error');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> settings() async {
    final root = TextEditingController(text: widget.document.assetDirectory);
    final wav = TextEditingController(text: widget.document.previewPath);
    await showDialog<void>(
        context: context,
        builder: (context) => AlertDialog(
              title: const Text('Patch audio files'),
              content: SingleChildScrollView(
                  child: SizedBox(
                      width: 560,
                      child: Column(mainAxisSize: MainAxisSize.min, children: [
                        const Text(
                            'Choose the folder containing your NAM and IR files. Node asset paths are relative to this folder.'),
                        TextField(
                            controller: root,
                            decoration: const InputDecoration(
                                labelText: 'Asset folder • absolute path')),
                        const SizedBox(height: 16),
                        TextField(
                            controller: wav,
                            decoration: const InputDecoration(
                                labelText: 'Preview WAV • absolute path',
                                helperText:
                                    'Leave blank for the bundled guitar clip')),
                        const SizedBox(height: 12),
                        const Text(
                            'Preview plays through the system audio output. Live patches use the engine’s selected audio interface. NAMs must match the patch sample rate.'),
                      ]))),
              actions: [
                TextButton(
                    onPressed: () => Navigator.pop(context),
                    child: const Text('Cancel')),
                FilledButton(
                    onPressed: () {
                      setState(() {
                        widget.document.assetDirectory = root.text.trim();
                        widget.document.previewPath = wav.text.trim();
                        widget.document.changed();
                      });
                      Navigator.pop(context);
                    },
                    child: const Text('Save'))
              ],
            ));
    WidgetsBinding.instance.addPostFrameCallback((_) {
      root.dispose();
      wav.dispose();
    });
  }

  @override
  Widget build(BuildContext context) {
    final valid = widget.document.validate() == null;
    return Padding(
        padding: const EdgeInsets.symmetric(horizontal: 12),
        child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
          Wrap(
              spacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                FilledButton.icon(
                    onPressed: !_busy && !_switching && _available && valid
                        ? () => run('apply_patch')
                        : null,
                    icon: const Icon(Icons.power_settings_new),
                    label: const Text('Apply to guitar')),
                OutlinedButton(
                    onPressed: !_busy && !_switching && _available
                        ? () => run('deactivate_patch')
                        : null,
                    child: const Text('Return to pedalboard')),
                OutlinedButton.icon(
                    onPressed: _player != null
                        ? () {
                            _player?.kill();
                            setState(() {
                              _player = null;
                              _message = 'Preview stopped';
                            });
                          }
                        : !_busy && valid
                            ? () => run('render_patch')
                            : null,
                    icon: Icon(_player != null ? Icons.stop : Icons.play_arrow),
                    label: Text(
                        _player != null ? 'Stop preview' : 'Preview clip')),
                IconButton(
                    tooltip: 'Patch audio files',
                    onPressed: _busy ? null : settings,
                    icon: const Icon(Icons.folder_open)),
                if (_busy)
                  const SizedBox(
                      width: 20,
                      height: 20,
                      child: CircularProgressIndicator(strokeWidth: 2)),
              ]),
          Text(_status,
              style:
                  TextStyle(color: _active ? LostChrome.lime : LostChrome.ice)),
          ListenableBuilder(
              listenable: live, builder: (context, _) => Text(live.label)),
          if (_message != null)
            Text(_message!, maxLines: 3, overflow: TextOverflow.ellipsis),
        ]));
  }
}
