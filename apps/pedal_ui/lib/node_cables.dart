part of 'main.dart';

/// Routes a patch cable between the visible ports, avoiding other nodes.
Path routePatchCable(Map<int, Offset> positions, int source, int destination) {
  final from = positions[source];
  final to = positions[destination];
  if (from == null || to == null) return Path();
  final start = from + const Offset(196, 70);
  final end = to + const Offset(24, 70);
  final reach = math.max(60.0, (end.dx - start.dx).abs() * .45);
  final direct = Path()
    ..moveTo(start.dx, start.dy)
    ..cubicTo(
        start.dx + reach, start.dy, end.dx - reach, end.dy, end.dx, end.dy);
  Rect nodeRect(Offset p) => p & const Size(220, 132);
  final otherRects = positions.entries
      .where((entry) => entry.key != source && entry.key != destination)
      .map((entry) => nodeRect(entry.value).inflate(12))
      .toList();
  // The ordinary curve is retained when it clears the intervening nodes and
  // exits/enters the two endpoint nodes through their adjacent edges.
  bool directClear() {
    for (final metric in direct.computeMetrics()) {
      final steps = math.max(1, (metric.length / 3).ceil());
      for (var i = 1; i < steps; i++) {
        final p =
            metric.getTangentForOffset(metric.length * i / steps)!.position;
        if (otherRects.any((r) => r.contains(p))) return false;
        if (nodeRect(from).contains(p) &&
            (p.dx < start.dx || (p.dy - start.dy).abs() > 2)) {
          return false;
        }
        if (nodeRect(to).contains(p) &&
            (p.dx > end.dx || (p.dy - end.dy).abs() > 2)) {
          return false;
        }
      }
    }
    return true;
  }

  if (directClear()) return direct;

  // Keep the initial and final leads clear of their own nodes. The extra
  // clearance around every rectangle allows corners to be rounded safely.
  final obstacles =
      positions.values.map((p) => nodeRect(p).inflate(18)).toList();
  final a = Offset(from.dx + 238, start.dy);
  final b = Offset(to.dx - 18, end.dy);
  final xs = <double>{a.dx, b.dx};
  final ys = <double>{a.dy, b.dy};
  for (final r in obstacles) {
    xs.addAll([r.left, r.right]);
    ys.addAll([r.top, r.bottom]);
  }
  final xList = xs.toList()..sort();
  final yList = ys.toList()..sort();
  final points = <Offset>[];
  final coordinates = <(int, int)>[];
  final grid =
      List.generate(xList.length, (_) => List<int>.filled(yList.length, -1));
  for (var x = 0; x < xList.length; x++) {
    for (var y = 0; y < yList.length; y++) {
      final p = Offset(xList[x], yList[y]);
      if (!obstacles.any((r) =>
          p.dx > r.left && p.dx < r.right && p.dy > r.top && p.dy < r.bottom)) {
        grid[x][y] = points.length;
        points.add(p);
        coordinates.add((x, y));
      }
    }
  }
  bool clear(Offset p, Offset q) => !obstacles.any((r) {
        if (p.dx == q.dx) {
          return p.dx > r.left &&
              p.dx < r.right &&
              math.max(p.dy, q.dy) > r.top &&
              math.min(p.dy, q.dy) < r.bottom;
        }
        return p.dy > r.top &&
            p.dy < r.bottom &&
            math.max(p.dx, q.dx) > r.left &&
            math.min(p.dx, q.dx) < r.right;
      });
  final startIndex = grid[xList.indexOf(a.dx)][yList.indexOf(a.dy)];
  final endIndex = grid[xList.indexOf(b.dx)][yList.indexOf(b.dy)];
  if (startIndex < 0 || endIndex < 0) return direct;
  // Heading is part of the search state: arriving at the same point from a
  // different direction can save a bend later. Prefer fewer bends first, then
  // shorter distance. Grid coordinates from other nodes must not cause zigzags.
  // Directions are east, south, west, north; both socket leads point east.
  final terminal = points.length * 4;
  final distance = List<double>.filled(terminal + 1, double.infinity);
  final bends = List<int>.filled(terminal + 1, 1 << 30);
  final previous = List<int>.filled(terminal + 1, -1);
  final heap = <(int, double, int)>[];
  bool less((int, double, int) a, (int, double, int) b) => a.$1 != b.$1
      ? a.$1 < b.$1
      : a.$2 != b.$2
          ? a.$2 < b.$2
          : a.$3 < b.$3;
  void push((int, double, int) value) {
    heap.add(value);
    var i = heap.length - 1;
    while (i > 0) {
      final parent = (i - 1) ~/ 2;
      if (!less(value, heap[parent])) break;
      heap[i] = heap[parent];
      i = parent;
    }
    heap[i] = value;
  }

  (int, double, int) pop() {
    final result = heap.first;
    final last = heap.removeLast();
    if (heap.isNotEmpty) {
      var i = 0;
      while (i * 2 + 1 < heap.length) {
        var child = i * 2 + 1;
        if (child + 1 < heap.length && less(heap[child + 1], heap[child])) {
          child++;
        }
        if (!less(heap[child], last)) break;
        heap[i] = heap[child];
        i = child;
      }
      heap[i] = last;
    }
    return result;
  }

  void relax(int state, int turns, double length, int parent) {
    if (turns < bends[state] ||
        (turns == bends[state] && length < distance[state])) {
      bends[state] = turns;
      distance[state] = length;
      previous[state] = parent;
      push((turns, length, state));
    }
  }

  final startState = startIndex * 4;
  relax(startState, 0, 0, -1);
  while (heap.isNotEmpty) {
    final (turns, cost, state) = pop();
    if (turns != bends[state] || cost != distance[state]) continue;
    if (state == terminal) break;
    final current = state ~/ 4, heading = state % 4;
    if (current == endIndex && heading != 2) {
      // Include the turn onto the final socket lead before choosing a winner.
      relax(terminal, turns + (heading == 0 ? 0 : 1), cost, state);
    }
    final (x, y) = coordinates[current];
    final neighbors = [(x + 1, y), (x, y + 1), (x - 1, y), (x, y - 1)];
    for (var direction = 0; direction < 4; direction++) {
      if (direction == (heading + 2) % 4) continue;
      final (nx, ny) = neighbors[direction];
      if (nx < 0 || nx >= xList.length || ny < 0 || ny >= yList.length) {
        continue;
      }
      final next = grid[nx][ny];
      if (next < 0 || !clear(points[current], points[next])) continue;
      relax(next * 4 + direction, turns + (direction == heading ? 0 : 1),
          cost + (points[current] - points[next]).distance, state);
    }
  }
  if (!distance[terminal].isFinite) return direct;
  final route = <Offset>[end];
  for (var state = previous[terminal]; state != -1; state = previous[state]) {
    route.add(points[state ~/ 4]);
  }
  route.add(start);
  // Only real corners need rounding; discard the intermediate grid stops.
  final ordered = <Offset>[];
  for (final p in route.reversed) {
    if (ordered.isNotEmpty && ordered.last == p) continue;
    while (ordered.length >= 2) {
      final first = ordered[ordered.length - 2], last = ordered.last;
      if ((first.dx == last.dx && last.dx == p.dx) ||
          (first.dy == last.dy && last.dy == p.dy)) {
        ordered.removeLast();
      } else {
        break;
      }
    }
    ordered.add(p);
  }
  final path = Path()..moveTo(start.dx, start.dy);
  for (var i = 1; i < ordered.length - 1; i++) {
    final before = ordered[i - 1];
    final corner = ordered[i];
    final after = ordered[i + 1];
    final inVector = corner - before;
    final outVector = after - corner;
    final radius =
        math.min(6.0, math.min(inVector.distance, outVector.distance) / 2);
    if (radius == 0) continue;
    final entry = corner - inVector / inVector.distance * radius;
    final exit = corner + outVector / outVector.distance * radius;
    path.lineTo(entry.dx, entry.dy);
    path.quadraticBezierTo(corner.dx, corner.dy, exit.dx, exit.dy);
  }
  path.lineTo(end.dx, end.dy);
  return path;
}
