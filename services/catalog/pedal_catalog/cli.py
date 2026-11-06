from __future__ import annotations

import argparse
import os
from pathlib import Path

from .index import scan_models
from .server import serve
from .backing_tracks import BackingTrackService
from .jev_ranker import JevRigRanker
from .tone3000 import DEFAULT_BASE_URL
from .tone3000_service import Tone3000Service
from .tone_maker import ToneMakerService
from .tone_memory import ToneMemoryService
from .transcription import TranscriptionService


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(prog="pedal-catalog")
    commands = root.add_subparsers(dest="command", required=True)
    scan = commands.add_parser("scan", help="index local .nam files")
    scan.add_argument("--models", type=Path, required=True)
    scan.add_argument("--database", type=Path, required=True)
    server = commands.add_parser("serve", help="run the local catalog socket service")
    server.add_argument("--models", type=Path, required=True)
    server.add_argument("--database", type=Path, required=True)
    server.add_argument("--socket", type=Path, required=True)
    server.add_argument(
        "--tone3000-key", default=os.environ.get("PEDAL_TONE3000_PUBLISHABLE_KEY")
    )
    server.add_argument(
        "--tone3000-redirect-uri",
        default=os.environ.get("PEDAL_TONE3000_REDIRECT_URI"),
    )
    server.add_argument(
        "--tone3000-token-file",
        type=Path,
        default=os.environ.get("PEDAL_TONE3000_TOKEN_FILE"),
    )
    server.add_argument(
        "--tone3000-base-url",
        default=os.environ.get("PEDAL_TONE3000_BASE_URL", DEFAULT_BASE_URL),
    )
    server.add_argument(
        "--openai-api-key", default=os.environ.get("PEDAL_OPENAI_API_KEY")
    )
    server.add_argument(
        "--openai-model", default=os.environ.get("PEDAL_OPENAI_MODEL", "gpt-5-mini")
    )
    server.add_argument(
        "--typesafe-api-key", default=os.environ.get("PEDAL_TYPESAFE_API_KEY")
    )
    server.add_argument(
        "--typesafe-model",
        default=os.environ.get("PEDAL_TYPESAFE_MODEL", "jev-latest"),
    )
    server.add_argument(
        "--stability-api-key", default=os.environ.get("PEDAL_STABILITY_API_KEY")
    )
    server.add_argument(
        "--riff-directory",
        type=Path,
        default=Path(os.environ.get("PEDAL_RIFF_DIRECTORY", "/var/lib/pedal/riffs")),
    )
    server.add_argument(
        "--backing-tracks-directory",
        type=Path,
        default=os.environ.get("PEDAL_BACKING_TRACK_DIRECTORY"),
    )
    server.add_argument(
        "--tone-memory-file",
        type=Path,
        default=os.environ.get("PEDAL_TONE_MEMORY_FILE"),
    )
    return root


def main() -> None:
    arguments = parser().parse_args()
    if arguments.command == "scan":
        records = scan_models(arguments.models, arguments.database)
        print(f"Indexed {len(records)} NAM model(s) into {arguments.database}")
    elif arguments.command == "serve":
        if bool(arguments.tone3000_key) != bool(arguments.tone3000_redirect_uri):
            raise SystemExit(
                "Both PEDAL_TONE3000_PUBLISHABLE_KEY and "
                "PEDAL_TONE3000_REDIRECT_URI are required"
            )
        tone3000 = None
        if arguments.tone3000_key:
            token_file = arguments.tone3000_token_file or (
                arguments.database.parent / "tone3000-tokens.json"
            )
            tone3000 = Tone3000Service(
                arguments.tone3000_key,
                arguments.tone3000_redirect_uri,
                token_file,
                base_url=arguments.tone3000_base_url,
            )
        tone_memory = ToneMemoryService(
            arguments.tone_memory_file or arguments.database.parent / "tone-memory.json"
        )
        tone_maker = ToneMakerService(
            arguments.openai_api_key or "",
            model=arguments.openai_model,
            tone_memory=tone_memory,
            jev_ranker=(
                JevRigRanker(arguments.typesafe_api_key, model=arguments.typesafe_model)
                if arguments.typesafe_api_key
                else None
            ),
        )
        backing_tracks = (
            BackingTrackService(
                arguments.stability_api_key,
                arguments.riff_directory,
                tracks_directory=arguments.backing_tracks_directory,
            )
            if arguments.stability_api_key
            else None
        )
        serve(
            arguments.models,
            arguments.database,
            arguments.socket,
            tone3000,
            tone_maker,
            backing_tracks,
            tone_memory,
            TranscriptionService(arguments.riff_directory),
        )


if __name__ == "__main__":
    main()
