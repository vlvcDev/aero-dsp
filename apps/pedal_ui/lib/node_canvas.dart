part of 'main.dart';

/// Layout belongs to the editor, never to the audio graph or render callback.
class PatchCanvas extends StatefulWidget {
  const PatchCanvas(
      {super.key,
      required this.document,
      required this.label,
      required this.onSelect,
      required this.onConnect,
      required this.onMoveStart,
      this.selected});
  final PatchDocument document;
  final String Function(Map) label;
  final ValueChanged<int> onSelect;
  final void Function(int, int) onConnect;
  final VoidCallback onMoveStart;
  final int? selected;
  @override
  State<PatchCanvas> createState() => _PatchCanvasState();
}

class _PatchCanvasState extends State<PatchCanvas> {
  static const _extent = 6000.0;
  static const _width = 220.0;
  static const _height = 132.0;
  final _view = TransformationController();
  final _viewport = GlobalKey();
  int? _connecting;
  Offset? _pointer;
  Offset _grab = Offset.zero;

  @override
  void dispose() {
    _view.dispose();
    super.dispose();
  }

  Offset world(Offset global) {
    final box = _viewport.currentContext!.findRenderObject()! as RenderBox;
    return _view.toScene(box.globalToLocal(global));
  }

  void zoom(double factor) {
    final box = _viewport.currentContext!.findRenderObject()! as RenderBox;
    final center = box.size.center(Offset.zero);
    final scene = _view.toScene(center);
    final scale = (_view.value.getMaxScaleOnAxis() * factor).clamp(.15, 2.0);
    _view.value = Matrix4.identity()
      ..translateByDouble(
          center.dx - scene.dx * scale, center.dy - scene.dy * scale, 0, 1)
      ..scaleByDouble(scale, scale, 1, 1);
  }

  void fit() {
    final box = _viewport.currentContext!.findRenderObject()! as RenderBox;
    final positions =
        widget.document.nodes.map((n) => widget.document.positions[n['id']]!);
    if (positions.isEmpty) return;
    final left = positions.map((p) => p.dx).reduce(math.min) - 24;
    final top = positions.map((p) => p.dy).reduce(math.min) - 24;
    final right = positions.map((p) => p.dx).reduce(math.max) + _width + 24;
    final bottom = positions.map((p) => p.dy).reduce(math.max) + _height + 24;
    final scale = math
        .min(box.size.width / (right - left), box.size.height / (bottom - top))
        .clamp(.15, 1.0);
    _view.value = Matrix4.identity()
      ..translateByDouble(-left * scale, -top * scale, 0, 1)
      ..scaleByDouble(scale, scale, 1, 1);
  }

  Widget socket(int id, bool output) {
    final active = output && _connecting == id;
    final icon = Container(
        width: 48,
        height: 44,
        alignment: Alignment.center,
        child: Icon(
            output ? Icons.radio_button_checked : Icons.radio_button_unchecked,
            size: 24,
            color: active ? LostChrome.lime : LostChrome.ice));
    if (output) {
      return Draggable<int>(
        key: ValueKey('patch-output-$id'),
        data: id,
        feedback: const Material(
            color: Colors.transparent,
            child: Icon(Icons.radio_button_checked,
                color: LostChrome.lime, size: 32)),
        onDragStarted: () => setState(() => _connecting = id),
        onDragUpdate: (details) =>
            setState(() => _pointer = world(details.globalPosition)),
        onDragEnd: (_) => setState(() {
          _connecting = null;
          _pointer = null;
        }),
        child: Tooltip(
            message: 'Output $id: drag to an input, or tap then tap an input',
            child: GestureDetector(
                onTap: () => setState(() {
                      _connecting = active ? null : id;
                      _pointer = null;
                    }),
                child: icon)),
      );
    }
    return DragTarget<int>(
      key: ValueKey('patch-input-$id'),
      onWillAcceptWithDetails: (details) =>
          widget.document.connectionError(details.data, id) == null,
      onAcceptWithDetails: (details) => widget.onConnect(details.data, id),
      builder: (context, candidates, rejected) => Tooltip(
          message: 'Input $id',
          child: InkWell(
            onTap: () {
              if (_connecting != null) widget.onConnect(_connecting!, id);
              setState(() {
                _connecting = null;
                _pointer = null;
              });
            },
            child: DecoratedBox(
                decoration: BoxDecoration(
                    color: candidates.isEmpty
                        ? Colors.transparent
                        : LostChrome.cobalt,
                    borderRadius: BorderRadius.circular(20)),
                child: icon),
          )),
    );
  }

  @override
  Widget build(BuildContext context) {
    final doc = widget.document;
    for (var i = 0; i < doc.nodes.length; i++) {
      final id = doc.nodes[i]['id'] as int;
      doc.positions.putIfAbsent(
          id, () => Offset(32 + (i % 8) * 300, 32 + (i ~/ 8) * 210));
    }
    final positions = Map<int, Offset>.from(doc.positions);
    final edges = <(int, int)>[
      for (final node in doc.nodes)
        for (final source in node['inputs'] as List)
          (source as int, node['id'] as int),
    ];
    return Column(children: [
      Row(children: [
        IconButton(
            tooltip: 'Zoom out',
            onPressed: () => zoom(1 / 1.2),
            icon: const Icon(Icons.remove)),
        IconButton(
            tooltip: 'Zoom in',
            onPressed: () => zoom(1.2),
            icon: const Icon(Icons.add)),
        IconButton(
            tooltip: 'Fit patch',
            onPressed: fit,
            icon: const Icon(Icons.fit_screen)),
        Expanded(
            child: Text(
                _connecting == null
                    ? 'Drag titles to move • connect sockets • tap a node to edit'
                    : 'Choose an input socket',
                style: const TextStyle(color: LostChrome.ice, fontSize: 12))),
        if (_connecting != null)
          IconButton(
              tooltip: 'Cancel connection',
              onPressed: () => setState(() {
                    _connecting = null;
                    _pointer = null;
                  }),
              icon: const Icon(Icons.close)),
      ]),
      Expanded(
          child: ClipRRect(
              borderRadius: BorderRadius.circular(16),
              child: ColoredBox(
                color: LostChrome.midnight,
                child: InteractiveViewer(
                  key: _viewport,
                  transformationController: _view,
                  constrained: false,
                  minScale: .15,
                  maxScale: 2,
                  alignment: Alignment.topLeft,
                  child: SizedBox(
                      width: _extent,
                      height: _extent,
                      child: Stack(children: [
                        Positioned.fill(
                            child: IgnorePointer(
                                child: CustomPaint(
                                    painter: _PatchCablePainter(positions,
                                        edges, _connecting, _pointer)))),
                        for (final node in doc.nodes)
                          Positioned(
                              key: ValueKey('patch-node-${node['id']}'),
                              left: positions[node['id']]!.dx,
                              top: positions[node['id']]!.dy,
                              width: _width,
                              height: _height,
                              child: Material(
                                  color: Colors.transparent,
                                  child: Container(
                                    decoration: BoxDecoration(
                                        borderRadius: BorderRadius.circular(16),
                                        border: Border.all(
                                            color: widget.selected == node['id']
                                                ? LostChrome.lime
                                                : LostChrome.ice),
                                        gradient: const LinearGradient(
                                            begin: Alignment.topLeft,
                                            end: Alignment.bottomRight,
                                            colors: [
                                              LostChrome.cobalt,
                                              LostChrome.navy
                                            ])),
                                    child: Column(children: [
                                      Draggable<Offset>(
                                          key: ValueKey(
                                              'patch-handle-${node['id']}'),
                                          data: doc.positions[node['id']],
                                          onDragStarted: widget.onMoveStart,
                                          feedback: const SizedBox.shrink(),
                                          dragAnchorStrategy: (_, __, global) {
                                            _grab = world(global) -
                                                doc.positions[node['id']]!;
                                            return Offset.zero;
                                          },
                                          onDragUpdate: (d) => setState(() {
                                                final position =
                                                    world(d.globalPosition) -
                                                        _grab;
                                                doc.positions[
                                                    node['id']
                                                        as int] = Offset(
                                                    position.dx.clamp(16,
                                                        _extent - _width - 16),
                                                    position.dy.clamp(
                                                        16,
                                                        _extent -
                                                            _height -
                                                            16));
                                              }),
                                          child: GestureDetector(
                                              behavior: HitTestBehavior.opaque,
                                              onTap: () => widget
                                                  .onSelect(node['id'] as int),
                                              child: SizedBox(
                                                  height: 48,
                                                  child: Row(children: [
                                                    const SizedBox(width: 10),
                                                    const Icon(
                                                        Icons.drag_indicator,
                                                        size: 18),
                                                    const SizedBox(width: 8),
                                                    Expanded(
                                                        child: Text(
                                                            widget.label(
                                                                node as Map),
                                                            style: const TextStyle(
                                                                fontWeight:
                                                                    FontWeight
                                                                        .bold,
                                                                color: LostChrome
                                                                    .pearl)))
                                                  ])))),
                                      SizedBox(
                                          height: 44,
                                          child: Row(
                                              mainAxisAlignment:
                                                  MainAxisAlignment
                                                      .spaceBetween,
                                              children: [
                                                if (node['processor']['type'] !=
                                                    'input')
                                                  socket(
                                                      node['id'] as int, false)
                                                else
                                                  const SizedBox(width: 48),
                                                const Text('STEREO',
                                                    style: TextStyle(
                                                        color: LostChrome.ice,
                                                        fontSize: 10)),
                                                if (node['processor']['type'] !=
                                                    'output')
                                                  socket(
                                                      node['id'] as int, true)
                                                else
                                                  const SizedBox(width: 48),
                                              ])),
                                      SizedBox(
                                          height: 38,
                                          child: TextButton(
                                              onPressed: () => widget
                                                  .onSelect(node['id'] as int),
                                              child: const Text('CONTROLS'))),
                                    ]),
                                  ))),
                      ])),
                ),
              ))),
    ]);
  }
}

class _PatchCablePainter extends CustomPainter {
  const _PatchCablePainter(
      this.positions, this.edges, this.connecting, this.pointer);
  final Map<int, Offset> positions;
  final List<(int, int)> edges;
  final int? connecting;
  final Offset? pointer;
  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()
      ..color = LostChrome.electric
      ..strokeWidth = 3
      ..style = PaintingStyle.stroke;
    void cable(Offset start, Offset end) {
      final reach = math.max(60.0, (end.dx - start.dx).abs() * .45);
      canvas.drawPath(
          Path()
            ..moveTo(start.dx, start.dy)
            ..cubicTo(start.dx + reach, start.dy, end.dx - reach, end.dy,
                end.dx, end.dy),
          paint);
    }

    for (final edge in edges) {
      final from = positions[edge.$1], to = positions[edge.$2];
      if (from != null && to != null) {
        cable(from + const Offset(196, 70), to + const Offset(24, 70));
      }
    }
    final from = positions[connecting];
    if (from != null && pointer != null) {
      paint.color = LostChrome.lime;
      cable(from + const Offset(196, 70), pointer!);
    }
  }

  @override
  bool shouldRepaint(covariant _PatchCablePainter oldDelegate) => true;
}
