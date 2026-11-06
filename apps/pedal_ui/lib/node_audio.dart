part of 'main.dart';

class PatchAudioControls extends StatefulWidget {
  const PatchAudioControls(
      {super.key, required this.document, required this.client});
  final PatchDocument document;
  final ControlClient client;
  @override
  State<PatchAudioControls> createState() => _PatchAudioControlsState();
}

class _PatchAudioControlsState extends State<PatchAudioControls> {
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
                        : 'PATCH LIVE • ${_rate ?? 0} Hz • ${status['latency_samples'] ?? 0} samples added'
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
    try {
      final response = await widget.client.patchRequest(command, payload);
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
          if (_message != null)
            Text(_message!, maxLines: 3, overflow: TextOverflow.ellipsis),
        ]));
  }
}
