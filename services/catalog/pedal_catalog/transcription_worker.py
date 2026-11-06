"""Isolated Basic Pitch 0.4 worker, invoked only for explicit user jobs."""
import json
import os
from pathlib import Path
import sys
import tempfile


def run(request_path, result_path):
    if hasattr(os, "nice"):
        os.nice(10)
    import numpy as np
    import soundfile as sf
    import onnxruntime as ort
    from basic_pitch import FilenameSuffix, build_icassp_2022_model_path
    from basic_pitch.inference import Model, predict
    from transcription import TUNINGS, assign_fingerings, tab_text

    request = json.loads(Path(request_path).read_text())
    tuning = TUNINGS[request["tuning"]]
    with sf.SoundFile(request["clean_path"]) as source:
        start = round(request["start_ms"] * source.samplerate / 1000)
        end = round(request["end_ms"] * source.samplerate / 1000)
        if source.channels not in (1, 2) or start >= len(source) or end > len(source) + source.samplerate // 1000:
            raise ValueError("Clip is outside the clean mono/stereo recording")
        source.seek(start)
        # Older vault captures stored clean DI in a stereo WAV. Basic Pitch
        # expects mono; collapse those copies instead of rejecting saved takes.
        audio = source.read(min(end, len(source)) - start, dtype="float32", always_2d=True).mean(axis=1)
        rate = source.samplerate
    if not np.isfinite(audio).all():
        raise ValueError("Recording contains invalid samples")
    # Explicit ONNX construction avoids TensorFlow/GPU selection and limits
    # worker CPU use. Model.predict supplies Basic Pitch's named output mapping.
    options = ort.SessionOptions()
    options.intra_op_num_threads = 1
    options.inter_op_num_threads = 1
    model = Model.__new__(Model)
    model.model_type = Model.MODEL_TYPES.ONNX
    model.model = ort.InferenceSession(
        str(build_icassp_2022_model_path(FilenameSuffix.onnx)),
        sess_options=options, providers=["CPUExecutionProvider"])
    with tempfile.TemporaryDirectory(prefix="aero-transcribe-") as directory:
        wav = Path(directory) / "clean.wav"
        sf.write(wav, audio, rate, subtype="FLOAT")
        _, midi, events = predict(
            str(wav), model_or_model_path=model,
            minimum_frequency=440 * 2 ** ((min(tuning) - 69) / 12),
            maximum_frequency=440 * 2 ** ((max(tuning) + 24 - 69) / 12),
            minimum_note_length=70, midi_tempo=request["bpm"])
    duration = len(audio) / rate
    events = [(max(0, s), min(duration, e), p, a, b)
              for s, e, p, a, b in events if s < duration and e > 0]
    if len(events) > 12000:
        raise ValueError("Too many detected notes; select a shorter clip")
    notes = assign_fingerings(events, tuning)
    text = tab_text(notes, tuning, request["bpm"])
    prefix = Path(request_path).with_suffix("")
    midi.write(str(prefix.with_suffix(".mid")))
    prefix.with_suffix(".txt").write_text(text)
    Path(result_path).write_text(json.dumps(dict(
        notes=notes, tab=text, midi_path=str(prefix.with_suffix(".mid")),
        tab_path=str(prefix.with_suffix(".txt")), duration_seconds=duration), allow_nan=False))


if __name__ == "__main__":
    run(*sys.argv[1:])
