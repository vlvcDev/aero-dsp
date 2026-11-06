"""Offline, opt-in guitar transcription; never runs on the audio thread."""
from __future__ import annotations

import hashlib
import json
import math
import os
from pathlib import Path
import re
import subprocess
import struct
import threading


TUNINGS = {
    "standard": [40, 45, 50, 55, 59, 64],
    "drop_d": [38, 45, 50, 55, 59, 64],
    "half_down": [39, 44, 49, 54, 58, 63],
    "whole_down": [38, 43, 48, 53, 57, 62],
    "drop_c": [36, 43, 48, 53, 57, 62],
}


class TranscriptionError(RuntimeError):
    pass


def validate_notes(notes, duration, tuning):
    if not isinstance(notes, list) or len(notes) > 12000:
        raise TranscriptionError("A transcription can contain at most 12000 notes")
    result = []
    for note in notes:
        if not isinstance(note, dict):
            raise TranscriptionError("Invalid note")
        start, end = note.get('start_seconds'), note.get('end_seconds')
        pitch, string, fret = note.get('midi'), note.get('string'), note.get('fret')
        if any(type(v) not in (int, float) or not math.isfinite(v) for v in (start, end)) or not 0 <= start < end <= duration + .001:
            raise TranscriptionError("Note times must lie within the clip")
        if type(pitch) is not int or not 0 <= pitch <= 127:
            raise TranscriptionError("MIDI pitch must be between 0 and 127")
        if string is not None or fret is not None:
            if type(string) is not int or type(fret) is not int or not (1 <= string <= 6 and 0 <= fret <= 24):
                raise TranscriptionError("Choose string 1–6 and fret 0–24")
            if tuning[6-string] + fret != pitch:
                raise TranscriptionError("Pitch does not match the selected string and fret")
        bends = note.get('bends', [])
        if not isinstance(bends, list) or len(bends) > 20000 or any(type(b) is not int or abs(b) > 120 for b in bends):
            raise TranscriptionError("Invalid pitch-motion data")
        strength = note.get('strength', .8)
        if type(strength) not in (int, float) or not math.isfinite(strength):
            raise TranscriptionError("Invalid note strength")
        result.append(dict(start_seconds=start, end_seconds=end, midi=pitch,
                           string=string, fret=fret, bends=bends, strength=max(0, min(1, strength))))
    return sorted(result, key=lambda n: (n['start_seconds'], n['midi']))


def corrected_midi(notes, bpm):
    """Dependency-free, type-0 MIDI export. Timing is not quantized to beats."""
    def vlq(value):
        data = [value & 127]
        while value > 127:
            value >>= 7
            data.insert(0, (value & 127) | 128)
        return bytes(data)
    tempo = round(60000000 / bpm)
    # High PPQ keeps MIDI export rounding below a millisecond even at 30 BPM.
    ppq = 9600
    events = []
    for n in notes:
        start = round(n['start_seconds'] * ppq * 1000000 / tempo)
        end = max(start + 1, round(n['end_seconds'] * ppq * 1000000 / tempo))
        events.extend([(start, 1, bytes([0x90, n['midi'], max(1, round(n['strength'] * 127))])),
                       (end, 0, bytes([0x80, n['midi'], 0]))])
    data = bytearray(b'\x00\xff\x51\x03' + tempo.to_bytes(3, 'big'))
    previous = 0
    for tick, _, event in sorted(events):
        data.extend(vlq(tick - previous) + event)
        previous = tick
    data.extend(b'\x00\xff\x2f\x00')
    return b'MThd' + struct.pack('>IHHH', 6, 0, 1, ppq) + b'MTrk' + struct.pack('>I', len(data)) + data


def assign_fingerings(events: list, tuning: list[int]) -> list[dict]:
    """Bounded beam search: preserve pitches and avoid overlapping a string.

    Unassignable notes stay visible with null string/fret rather than silently
    removing pitches. This is a fingering suggestion, not string recognition.
    """
    notes = []
    for start, end, pitch, amplitude, bends in events:
        if not all(math.isfinite(float(x)) for x in (start, end, pitch, amplitude)):
            continue
        if end <= start:
            continue
        notes.append(dict(start_seconds=float(start), end_seconds=float(end),
                          midi=int(pitch), strength=float(amplitude),
                          bends=[] if bends is None else [int(b) for b in bends]))
    notes.sort(key=lambda n: (n["start_seconds"], n["midi"]))
    # score, occupied-until per string, active frets, last hand position, path
    beam = [(0.0, (0.0,) * 6, (0,) * 6, 3.0, None)]
    for note in notes:
        expanded = []
        for score, ends, frets, hand, path in beam:
            choices = []
            for string, open_pitch in enumerate(tuning):
                fret = note["midi"] - open_pitch
                if not 0 <= fret <= 24 or ends[string] > note["start_seconds"] + .015:
                    continue
                held = [frets[i] for i in range(6)
                        if ends[i] > note["start_seconds"] + .015 and frets[i] > 0]
                span = max(held + [fret]) - min(held + [fret]) if held and fret else 0
                if span > 5:
                    continue
                cost = (abs(fret - hand) * .3 if fret else .15) + fret * .025 + span * .2
                new_ends, new_frets = list(ends), list(frets)
                new_ends[string], new_frets[string] = note["end_seconds"], fret
                choices.append((score + cost, tuple(new_ends), tuple(new_frets),
                                float(fret) if fret else hand,
                                (path, 6 - string, fret)))
            expanded.extend(choices)
            expanded.append((score + 20, ends, frets, hand, (path, None, None)))
        beam = sorted(expanded, key=lambda candidate: candidate[0])[:24]
    path = beam[0][4]
    for note in reversed(notes):
        path, note["string"], note["fret"] = path
    return notes


def tab_text(notes: list[dict], tuning: list[int], bpm: int) -> str:
    """Readable, unquantized six-string draft, with explicit onset timestamps."""
    groups = []
    for note in notes:
        if (not groups or abs(note["start_seconds"] - groups[-1][0]) > .03
                or (note.get('string') is not None and
                    any(n.get('string') == note['string'] for n in groups[-1][1]))):
            groups.append((note["start_seconds"], []))
        groups[-1][1].append(note)
    pitch_names = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
    lines = [f"DRAFT TAB — {bpm} BPM reference — unquantized", "Columns = attacks, not equal time. t = seconds from clip start.",
             "Suggested fingerings; ~ = detected pitch motion, not a verified technique.", ""]
    for offset in range(0, len(groups), 8):
        block = groups[offset:offset + 8]
        lines.append("t  " + "".join(f"{time:7.2f}" for time, _ in block))
        for string in range(1, 7):
            row = f"{pitch_names[tuning[6-string] % 12]:2}|"
            for _, group in block:
                found = next((n for n in group if n["string"] == string), None)
                cell = str(found["fret"]) + ("~" if found["bends"] else "") if found else ""
                row += cell.center(7, "-")
            lines.append(row + "|")
        lines.append("")
    unassigned = sum(n["string"] is None for n in notes)
    lines.append(f"{len(notes)} detected notes; {unassigned} need manual fingering review.")
    for note in notes:
        if note['string'] is None:
            lines.append(f"Review: {note['start_seconds']:.3f}s, MIDI {note['midi']}, no playable fingering assigned.")
    return "\n".join(lines)


class TranscriptionService:
    def __init__(self, riff_directory: Path, *, python: str | None = None):
        self.riffs = riff_directory.resolve()
        self.directory = self.riffs / "transcriptions"
        self.python = python or os.environ.get("PEDAL_TRANSCRIPTION_PYTHON") or str(
            Path(__file__).resolve().parents[3] / ".transcription-venv/bin/python")
        self.lock = threading.Lock()
        self.active: str | None = None

    def status(self):
        return {"available": Path(self.python).is_file(), "engine": "Spotify Basic Pitch",
                "setup": "Run bash tools/setup-transcription.sh, then reopen this window.",
                "tunings": list(TUNINGS)}

    def _write(self, job):
        self.directory.mkdir(parents=True, exist_ok=True)
        target = self.directory / f"{job['id']}.json"
        temporary = target.with_suffix(".tmp")
        temporary.write_text(json.dumps(job, allow_nan=False))
        temporary.replace(target)

    def job(self, identifier):
        if not isinstance(identifier, str) or not re.fullmatch(r"[a-f0-9]{32}", identifier):
            raise TranscriptionError("Invalid transcription identifier")
        with self.lock:
            try:
                job = json.loads((self.directory / f"{identifier}.json").read_text())
            except (OSError, ValueError) as error:
                raise TranscriptionError("Saved transcription not found") from error
            if job["status"] == "running" and self.active != identifier:
                job.update(status="failed", error="Transcription was interrupted. Generate again to retry.")
            return self._present(job)

    def _present(self, job):
        result = dict(job)
        if job.get('status') == 'complete':
            result['original_notes'] = job.get('notes', [])
            result['notes'] = job.get('edited_notes', job.get('notes', []))
            result['revision'] = job.get('revision', 0)
            if job.get('revision', 0):
                result['tab'] = tab_text(result['notes'], TUNINGS[job['tuning']], job['bpm'])
                prefix = self.directory / f"{job['id']}-edit-{job['revision']}"
                result.update(tab_path=str(prefix.with_suffix('.txt')), midi_path=str(prefix.with_suffix('.mid')))
        return result

    def save(self, payload):
        if not isinstance(payload, dict):
            raise TranscriptionError("Invalid edit request")
        current = self.job(payload.get('id'))
        if current['status'] != 'complete':
            raise TranscriptionError("Wait for transcription to finish")
        notes = validate_notes(payload.get('notes'), current['duration_seconds'], TUNINGS[current['tuning']])
        with self.lock:
            job = json.loads((self.directory / f"{current['id']}.json").read_text())
            if type(payload.get('revision')) is not int or payload['revision'] != job.get('revision', 0):
                raise TranscriptionError("This tab changed in another window. Reopen it before saving.")
            job.update(edited_notes=notes, revision=job.get('revision', 0) + 1)
            prefix = self.directory / f"{job['id']}-edit-{job['revision']}"
            # Immutable export filenames; only publish the revision once both
            # exports are complete. Original inference files are never changed.
            prefix.with_suffix('.txt').write_text(tab_text(notes, TUNINGS[job['tuning']], job['bpm']))
            prefix.with_suffix('.mid').write_bytes(corrected_midi(notes, job['bpm']))
            self._write(job)
            return self._present(job)

    def queue(self, payload):
        if not isinstance(payload, dict):
            raise TranscriptionError("Transcription options must be an object")
        riff_id = payload.get("riff_id")
        tuning = payload.get("tuning", "standard")
        start, end, bpm = payload.get("start_ms"), payload.get("end_ms"), payload.get("bpm", 120)
        if not isinstance(riff_id, str) or not re.fullmatch(r"[A-Za-z0-9_-]+", riff_id):
            raise TranscriptionError("Choose a saved guitar recording")
        if not isinstance(tuning, str) or tuning not in TUNINGS:
            raise TranscriptionError("Unsupported guitar tuning")
        if any(type(v) is not int for v in (start, end, bpm)) or not (0 <= start < end and end-start <= 180000 and 30 <= bpm <= 300):
            raise TranscriptionError("Choose a clip of up to 3 minutes and a BPM from 30 to 300")
        try:
            metadata = (self.riffs / f"{riff_id}.json").resolve()
            metadata.relative_to(self.riffs)
            riff = json.loads(metadata.read_text())
            if not isinstance(riff, dict) or riff.get("id") != riff_id:
                raise ValueError("Riff identifier mismatch")
            source = Path(riff["clean_path"]).resolve()
            source.relative_to(self.riffs)
            if payload.get("clean_path") and Path(payload["clean_path"]).resolve() != source:
                raise ValueError("Clip source differs from saved riff")
            stat = source.stat()
            if stat.st_size > 256 * 1024 * 1024:
                raise ValueError("Recording too large")
        except (OSError, ValueError, KeyError, TypeError) as error:
            raise TranscriptionError("The original clean guitar recording is unavailable or invalid") from error
        spec = dict(riff_id=riff_id, clean_path=str(source), start_ms=start, end_ms=end,
                    bpm=bpm, tuning=tuning, source_size=stat.st_size, source_mtime=stat.st_mtime_ns,
                    version=1)
        identifier = hashlib.sha256(json.dumps(spec, sort_keys=True).encode()).hexdigest()[:32]
        with self.lock:
            target = self.directory / f"{identifier}.json"
            if target.exists():
                previous = json.loads(target.read_text())
                if previous["status"] == "complete" or self.active == identifier:
                    return self._present(previous)
            if self.active:
                raise TranscriptionError("Another clip is being transcribed. Wait for it to finish.")
            if not self.status()["available"]:
                raise TranscriptionError(self.status()["setup"])
            job = dict(spec, id=identifier, status="running")
            self._write(job)
            self.active = identifier
            threading.Thread(target=self._run, args=(job,), daemon=True).start()
            return dict(job)

    def _run(self, job):
        result = self.directory / f"{job['id']}.result"
        try:
            # Do not pass cloud secrets to this offline worker. Limit numerical
            # library threads; ONNX is additionally restricted in the worker.
            env = {key: value for key, value in os.environ.items()
                   if key in ("PATH", "HOME", "TMPDIR", "LANG", "LD_LIBRARY_PATH")}
            env.update(OMP_NUM_THREADS="1", OPENBLAS_NUM_THREADS="1", NUMBA_NUM_THREADS="1")
            completed = subprocess.run(
                [self.python, str(Path(__file__).with_name("transcription_worker.py")),
                 str(self.directory / f"{job['id']}.json"), str(result)],
                env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                timeout=900, text=True)
            if completed.returncode:
                (self.directory / f"{job['id']}.log").write_text(completed.stderr[-8000:])
                raise TranscriptionError("Basic Pitch could not transcribe this clip. Check the worker installation and audio file.")
            output = json.loads(result.read_text())
            job.update(status="complete", **output)
        except subprocess.TimeoutExpired:
            job.update(status="failed", error="Transcription exceeded 15 minutes. Try a shorter clip.")
        except Exception as error:
            job.update(status="failed", error=str(error))
        finally:
            with self.lock:
                try:
                    self._write(job)
                finally:
                    self.active = None
                    result.unlink(missing_ok=True)
