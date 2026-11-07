part of 'main.dart';

/// An independent rig snapshot and the folder containing its assets.
class ToneWorkbenchResult {
  ToneWorkbenchResult(Map<String, dynamic> document, this.assetDirectory)
      : document = jsonDecode(jsonEncode(document)) as Map<String, dynamic>;
  final Map<String, dynamic> document;
  final String assetDirectory;
}

class _ToneWorkbenchSession {
  ToneWorkbenchResult? a, b;
  String source = 'built_in', path = '';
}

final _toneWorkbenchSessions = Expando<_ToneWorkbenchSession>();

/// Pops with a result when the user chooses to recall a snapshot.
class ToneWorkbench extends StatefulWidget {
  const ToneWorkbench(
      {super.key,
      required this.document,
      required this.client,
      this.showHeader = true});
  final PatchDocument document;
  final ControlClient client;
  final bool showHeader;
  @override
  State<ToneWorkbench> createState() => _ToneWorkbenchState();
}

class _ToneWorkbenchState extends State<ToneWorkbench> {
  late final _ToneWorkbenchSession session;
  late final TextEditingController path;
  Process? player;
  Map<String, dynamic>? rendered;
  bool busy = false;
  String? message;

  @override
  void initState() {
    super.initState();
    session =
        _toneWorkbenchSessions[widget.document] ??= _ToneWorkbenchSession();
    path = TextEditingController(text: session.path);
  }

  @override
  void dispose() {
    player?.kill();
    path.dispose();
    super.dispose();
  }

  void capture(bool a) {
    final error = widget.document.validate();
    if (error != null) {
      setState(() => message = error);
      return;
    }
    setState(() {
      final snapshot = ToneWorkbenchResult(
          widget.document.data, widget.document.assetDirectory);
      if (a) {
        session.a = snapshot;
      } else {
        session.b = snapshot;
      }
      rendered = null;
      message = 'Captured current draft as ${a ? 'A' : 'B'}.';
    });
  }

  void stop() {
    player?.kill();
    setState(() {
      player = null;
      message = 'Preview stopped.';
    });
  }

  Future<void> play(String slot) async {
    player?.kill();
    setState(() {
      player = null;
      busy = true;
      message = 'Preparing comparison…';
    });
    try {
      if (rendered == null) {
        final a = session.a!, b = session.b!;
        rendered = await widget.client.patchRequest('render_patch_comparison', {
          'a': {'document': a.document, 'directory': a.assetDirectory},
          'b': {'document': b.document, 'directory': b.assetDirectory},
          'source': session.source,
          if (session.source == 'file') 'path': path.text.trim(),
        });
      }
      if (!mounted) return;
      final process = await _startLocalAudioPlayback(
          rendered!['${slot.toLowerCase()}_path'] as String);
      if (!mounted) {
        process.kill();
        return;
      }
      setState(() {
        player = process;
        message = rendered!['matched'] == true
            ? 'Playing $slot • matched levels'
            : 'Playing $slot • levels could not be matched';
      });
      unawaited(process.stdout.drain());
      unawaited(process.stderr.drain());
      unawaited(process.exitCode.then((exit) {
        if (mounted && identical(player, process)) {
          setState(() {
            player = null;
            if (exit != 0) message = 'Playback failed ($exit).';
          });
        }
      }));
    } catch (error) {
      if (mounted) setState(() => message = '$error');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Widget slot(String name, ToneWorkbenchResult? snapshot) => Card(
        child: Padding(
          padding: const EdgeInsets.all(12),
          child:
              Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text('Rig $name', style: Theme.of(context).textTheme.titleLarge),
            Text(snapshot == null
                ? 'Empty'
                : '${(snapshot.document['graph']['nodes'] as List).length} nodes • ${snapshot.document['sample_rate']} Hz'),
            Wrap(spacing: 8, runSpacing: 4, children: [
              OutlinedButton.icon(
                  onPressed: busy ? null : () => capture(name == 'A'),
                  icon: const Icon(Icons.save_alt),
                  label: const Text('Capture draft')),
              FilledButton.icon(
                  onPressed: busy || session.a == null || session.b == null
                      ? null
                      : () => play(name),
                  icon: const Icon(Icons.play_arrow),
                  label: Text('Play $name')),
              TextButton(
                  onPressed: busy || snapshot == null
                      ? null
                      : () => Navigator.pop(
                          context,
                          ToneWorkbenchResult(
                              snapshot.document, snapshot.assetDirectory)),
                  child: const Text('Recall to draft')),
            ]),
          ]),
        ),
      );

  @override
  Widget build(BuildContext context) => SafeArea(
        child: Padding(
          padding: const EdgeInsets.all(16),
          child:
              Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
            if (widget.showHeader)
              Row(children: [
                Expanded(
                    child: Text('Tone Workbench',
                        style: Theme.of(context).textTheme.headlineSmall)),
                IconButton(
                    tooltip: 'Close',
                    onPressed: () => Navigator.pop(context),
                    icon: const Icon(Icons.close))
              ]),
            Expanded(
                child: SingleChildScrollView(
                    child: Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                  const Text(
                      'Capture two drafts, then compare them through the same clean clip. Recall a winner to edit, save, or apply it.'),
                  DropdownButtonFormField<String>(
                    value: session.source,
                    decoration: const InputDecoration(labelText: 'Clean clip'),
                    items: const [
                      DropdownMenuItem(
                          value: 'built_in',
                          child: Text('Bundled guitar clip')),
                      DropdownMenuItem(value: 'file', child: Text('WAV file')),
                      DropdownMenuItem(
                          value: 'latest_clean',
                          child: Text('Latest saved clean take'))
                    ],
                    onChanged: busy
                        ? null
                        : (value) => setState(() {
                              session.source = value!;
                              rendered = null;
                            }),
                  ),
                  if (session.source == 'file')
                    TextField(
                        controller: path,
                        enabled: !busy,
                        onChanged: (value) {
                          session.path = value;
                          rendered = null;
                        },
                        decoration:
                            const InputDecoration(labelText: 'WAV file path')),
                  slot('A', session.a),
                  slot('B', session.b),
                  if (player != null)
                    Align(
                        alignment: Alignment.centerLeft,
                        child: OutlinedButton.icon(
                            onPressed: stop,
                            icon: const Icon(Icons.stop),
                            label: const Text('Stop'))),
                  if (message != null)
                    Padding(
                        padding: const EdgeInsets.only(top: 8),
                        child: Text(message!)),
                  if (busy) const LinearProgressIndicator(),
                ]))),
          ]),
        ),
      );
}
