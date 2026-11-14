import 'dart:math' as math;
import 'dart:ui';

import 'package:flutter_test/flutter_test.dart';
import 'package:pedal_ui/main.dart';

void main() {
  void checkRoute(Map<int, Offset> nodes, int from, int to) {
    final path = routePatchCable(nodes, from, to);
    final metric = path.computeMetrics().single;
    final start = nodes[from]! + const Offset(196, 70);
    final end = nodes[to]! + const Offset(24, 70);
    expect(metric.getTangentForOffset(0)!.position, start);
    expect((metric.getTangentForOffset(metric.length)!.position - end).distance,
        lessThan(.1));
    for (var d = 0.0; d < metric.length; d += 1) {
      final p = metric.getTangentForOffset(d)!.position;
      for (final entry in nodes.entries) {
        if (entry.key == from || entry.key == to) continue;
        expect((entry.value & const Size(220, 132)).inflate(10).contains(p),
            isFalse,
            reason: 'Cable intersects node ${entry.key} at $p');
      }
    }
  }

  // Rounded orthogonal corners each contribute a quarter turn. Sampling
  // tangent changes measures the real path rather than its control points.
  void expectBends(Map<int, Offset> nodes, int from, int to, int bends) {
    final metric = routePatchCable(nodes, from, to).computeMetrics().single;
    var previous = metric.getTangentForOffset(0)!.angle;
    var turning = 0.0;
    for (var d = .25; d < metric.length; d += .25) {
      final angle = metric.getTangentForOffset(d)!.angle;
      var delta = angle - previous;
      while (delta > math.pi) {
        delta -= 2 * math.pi;
      }
      while (delta < -math.pi) {
        delta += 2 * math.pi;
      }
      turning += delta.abs();
      previous = angle;
    }
    expect(turning / (math.pi / 2), closeTo(bends, .15));
    checkRoute(nodes, from, to);
  }

  test('retains the ordinary curve between unobstructed ports', () {
    const nodes = {0: Offset(32, 32), 1: Offset(332, 32)};
    final metric = routePatchCable(nodes, 0, 1).computeMetrics().single;
    expect(metric.length, closeTo(128, .1));
    checkRoute(nodes, 0, 1);
  });
  test('routes dry bypass around the effect node', () {
    expectBends(
        const {0: Offset(32, 32), 1: Offset(332, 32), 2: Offset(632, 32)},
        0,
        2,
        4);
  });
  test('routes around multiple obstacles', () {
    checkRoute(const {
      0: Offset(32, 32),
      1: Offset(332, 32),
      2: Offset(632, 32),
      3: Offset(932, 32)
    }, 0, 3);
  });
  test('uses one outside lane around staggered blockers after small moves', () {
    for (final shift in [0.0, 1.0, 3.0, 6.0]) {
      expectBends({
        0: const Offset(32, 32),
        1: Offset(332, 32 + shift),
        2: Offset(632, 62 - shift),
        3: const Offset(932, 32),
      }, 0, 3, 4);
    }
  });
  test('keeps an unobstructed slightly offset connection smooth', () {
    const nodes = {0: Offset(32, 32), 1: Offset(332, 35)};
    final metric = routePatchCable(nodes, 0, 1).computeMetrics().single;
    expect(metric.length, lessThan(129));
    checkRoute(nodes, 0, 1);
  });
  test('routes wrapped connections around unrelated nodes', () {
    expectBends(const {
      0: Offset(32, 32),
      1: Offset(332, 32),
      2: Offset(632, 32),
      3: Offset(32, 232),
      4: Offset(332, 232),
      5: Offset(632, 232)
    }, 2, 3, 4);
  });
  test('returns an empty path for missing nodes', () {
    expect(routePatchCable(const {0: Offset.zero}, 0, 1).computeMetrics(),
        isEmpty);
  });
}
