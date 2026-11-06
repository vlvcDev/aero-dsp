part of 'main.dart';

class CaptureCarousel extends StatefulWidget {
  const CaptureCarousel(
      {super.key,
      required this.catalog,
      required this.path,
      required this.onSelected});
  final CatalogClient catalog;
  final String? path;
  final Future<void> Function(CatalogModel) onSelected;
  @override
  State<CaptureCarousel> createState() => _CaptureCarouselState();
}

class _CaptureCarouselState extends State<CaptureCarousel> {
  List<CatalogModel> _captures = [];
  int _index = 0;
  bool _busy = false;
  String? _error;
  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final models = await widget.catalog.listModels();
      final current = models.where((m) => m.path == widget.path).firstOrNull;
      final group = current?.captureGroup;
      if (!mounted) return;
      setState(() {
        _captures = group == null
            ? []
            : models.where((m) => m.captureGroup == group).toList()
          ..sort((a, b) => a.name.compareTo(b.name));
        _index =
            math.max(0, _captures.indexWhere((m) => m.path == widget.path));
        _error = null;
      });
    } catch (error) {
      if (mounted) setState(() => _error = '$error');
    }
  }

  Future<void> _choose(int index) async {
    if (_busy || _captures.isEmpty) return;
    final target = (index + _captures.length) % _captures.length;
    if (target == _index) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await widget.onSelected(_captures[target]);
      if (mounted) setState(() => _index = target);
    } catch (error) {
      if (mounted) setState(() => _error = 'Capture was not changed: $error');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _list() async {
    final index = await showDialog<int>(
        context: context,
        builder: (context) => SimpleDialog(
              title: const Text('Cached captures'),
              children: [
                for (var i = 0; i < _captures.length; i++)
                  SimpleDialogOption(
                    onPressed: () => Navigator.pop(context, i),
                    child:
                        Text('${i == _index ? '● ' : ''}${_captures[i].name}'),
                  )
              ],
            ));
    if (index != null && mounted) await _choose(index);
  }

  @override
  Widget build(BuildContext context) {
    if (_captures.isEmpty && _error == null) return const SizedBox.shrink();
    return Column(mainAxisSize: MainAxisSize.min, children: [
      if (_captures.isNotEmpty)
        GestureDetector(
          onHorizontalDragEnd: _busy
              ? null
              : (details) {
                  final speed = details.primaryVelocity ?? 0;
                  if (speed.abs() > 50) _choose(_index + (speed < 0 ? 1 : -1));
                },
          child: Container(
            decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(12),
                gradient: const LinearGradient(
                    colors: [LostChrome.ocean, LostChrome.navy]),
                border: Border.all(color: LostChrome.ice)),
            child: Row(children: [
              IconButton(
                  tooltip: 'Previous capture',
                  onPressed: _busy || _captures.length < 2
                      ? null
                      : () => _choose(_index - 1),
                  icon: const Icon(Icons.chevron_left)),
              Expanded(
                  child: TextButton(
                      onPressed: _busy ? null : _list,
                      child: Text(
                          '${_captures[_index].name}\n${_index + 1} / ${_captures.length} cached captures',
                          maxLines: 3,
                          overflow: TextOverflow.ellipsis,
                          textAlign: TextAlign.center,
                          style: const TextStyle(
                              color: LostChrome.pearl,
                              fontWeight: FontWeight.w700)))),
              IconButton(
                  tooltip: 'Next capture',
                  onPressed: _busy || _captures.length < 2
                      ? null
                      : () => _choose(_index + 1),
                  icon: const Icon(Icons.chevron_right)),
            ]),
          ),
        ),
      if (_busy) const LinearProgressIndicator(),
      if (_error != null)
        Text(_error!,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(color: Color(0xffffaabd))),
    ]);
  }
}
