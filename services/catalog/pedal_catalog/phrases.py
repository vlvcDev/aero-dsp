"""Versioned beat-relative guitar phrases: harmony stays separate from performance."""
import array
import hashlib
import json
import math
import os
import sys
import tempfile
import wave
from pathlib import Path
from .chords import BY_ID, ROOTS, TUNING, ChordError, validate_steps

PPQ = 480
PATTERNS = ('block', 'up', 'down', 'updown', 'skip', 'pulse', 'offbeat', 'chug')
MODES = {'unspecified': None, 'major': (0, 2, 4, 5, 7, 9, 11),
         'minor': (0, 2, 3, 5, 7, 8, 10), 'dorian': (0, 2, 3, 5, 7, 9, 10),
         'mixolydian': (0, 2, 4, 5, 7, 9, 10)}


def build_phrase(steps, key='C', mode='unspecified'):
    steps = validate_steps(steps)
    if key not in ROOTS or not isinstance(mode, str) or mode not in MODES:
        raise ChordError('Choose a supported key and mode')
    scale = None if MODES[mode] is None else {(ROOTS.index(key) + n) % 12 for n in MODES[mode]}
    notes, spans, cursor, outside = [], [], 0, set()
    for index, step in enumerate(steps):
        pattern, division = step.get('pattern', 'block'), step.get('division', 2)
        shape = BY_ID[step['chord']]['shapes'][step['shape']]
        strings = [(s, f, TUNING[s] + f) for s, f in enumerate(shape['frets']) if f >= 0]
        # Low-to-high guitar string ordering, not arbitrary MIDI sorting.
        order = list(range(len(strings)))
        if pattern == 'down': order.reverse()
        if pattern == 'updown': order += order[-2:0:-1]
        if pattern == 'skip': order = order[::2] + order[1::2]
        length, quantum = step['beats'] * PPQ, PPQ // division
        spans.append({'step': index, 'chord': step['chord'], 'tick': cursor, 'duration': length})
        attacks = [0] if pattern == 'block' else range(0, length, quantum)
        for count, start in enumerate(attacks):
            if pattern == 'offbeat' and count % 2 == 0: continue
            sounding = strings if pattern in ('block', 'pulse', 'offbeat') else (
                strings[:2] if pattern == 'chug' else [strings[order[count % len(order)]]])
            gate = 1.0 if pattern == 'block' else .3 if pattern == 'chug' else .8
            duration = min(length - start, round((length if pattern == 'block' else quantum) * gate))
            for string, fret, midi in sounding:
                if scale is not None and midi % 12 not in scale: outside.add(ROOTS[midi % 12])
                notes.append({'tick': cursor + start, 'duration': duration, 'string': string,
                              'fret': fret, 'midi': midi, 'velocity': .85 if start % PPQ == 0 else .65,
                              'articulation': 'muted' if pattern == 'chug' else 'normal', 'step': index})
        cursor += length
    return {'version': 1, 'ppq': PPQ, 'key': key, 'mode': mode, 'duration_ticks': cursor,
            'steps': steps, 'chords': spans, 'notes': notes,
            'outside_key': sorted(outside), 'time_signature': [4, 4]}


def render_phrase(directory: Path, steps, bpm, key='C', mode='unspecified'):
    if type(bpm) is not int or not 40 <= bpm <= 240:
        raise ChordError('Use 40–240 BPM')
    phrase = build_phrase(steps, key, mode)
    rate = 22050
    frames_per_tick = rate * 60 / bpm / PPQ
    total = round(phrase['duration_ticks'] * frames_per_tick)
    samples = array.array('f', [0]) * total
    # Bounded gain: at most six strings per attack; events don't overlap the next attack.
    for note in phrase['notes']:
        start = round(note['tick'] * frames_per_tick)
        count = max(1, round((note['tick'] + note['duration']) * frames_per_tick) - start)
        phase = 2 * math.pi * 440 * 2 ** ((note['midi'] - 69) / 12) / rate
        decay = 16 if note['articulation'] == 'muted' else 2.5
        for j in range(min(count, total - start)):
            env = min(1, j / (rate * .006)) * min(1, (count - 1 - j) / (rate * .025))
            env *= math.exp(-decay * j / rate)
            samples[start + j] += .065 * note['velocity'] * env * (math.sin(phase * j) + .18 * math.sin(2 * phase * j))
    directory.mkdir(parents=True, exist_ok=True)
    target = directory / 'phrase-preview.wav'
    fd, temp = tempfile.mkstemp(prefix='phrase-', suffix='.wav', dir=directory)
    try:
        pcm = array.array('h', (round(max(-1, min(1, x)) * 32767) for x in samples))
        if sys.byteorder != 'little': pcm.byteswap()
        with os.fdopen(fd, 'wb') as stream, wave.open(stream, 'wb') as wav:
            wav.setnchannels(1); wav.setsampwidth(2); wav.setframerate(rate)
            wav.writeframes(pcm.tobytes())
        os.replace(temp, target)
    finally:
        Path(temp).unlink(missing_ok=True)
    return {'path': str(target.resolve()), 'duration_ms': round(total / rate * 1000),
            'fingerprint': hashlib.sha256(json.dumps([phrase, bpm]).encode()).hexdigest(), 'phrase': phrase}
