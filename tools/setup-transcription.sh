#!/usr/bin/env bash
set -euo pipefail
repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
command -v uv >/dev/null || { echo 'Install uv first: https://docs.astral.sh/uv/'; exit 1; }
if [[ ! -x "$repo_dir/.transcription-venv/bin/python" ]]; then
  uv venv --python 3.11 "$repo_dir/.transcription-venv"
fi
# Basic Pitch's published Linux metadata forces TensorFlow even with [onnx].
# Install its explicit inference dependencies, then the unchanged package/model
# without that heavyweight, unused backend. This environment is worker-only.
uv pip install --python "$repo_dir/.transcription-venv/bin/python" \
  'numpy<2' 'librosa>=0.10,<0.12' 'mir_eval>=0.6,<0.9' \
  'pretty_midi>=0.2.9,<0.3' 'resampy>=0.2.2,<0.4.3' \
  'scikit-learn<2' 'scipy<2' typing_extensions 'onnxruntime>=1.17,<2' soundfile 'setuptools<81'
uv pip install --python "$repo_dir/.transcription-venv/bin/python" --no-deps 'basic-pitch==0.4.0'
"$repo_dir/.transcription-venv/bin/python" -c 'from basic_pitch.inference import Model; from basic_pitch import FilenameSuffix, build_icassp_2022_model_path; Model(build_icassp_2022_model_path(FilenameSuffix.onnx)); print("Basic Pitch ready — no API key required.")'
