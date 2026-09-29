"""Fast, confidence-aware TONE3000 metadata ranking with TypeSafe Jev.

Jev is intentionally used *after* the free-form planner and TONE3000 search.
It never chooses search terms, downloads a model, or touches the real-time
audio process. Its job is a bounded decision: score the small set of catalog
metadata pairs that the player may choose from.
"""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any, Mapping, Protocol, Sequence


TYPESAFE_SYSTEMONE_URL = "https://api.typesafe.ai/v1/systemone"
DEFAULT_JEV_MODEL = "jev-latest"
DEFAULT_MIN_CONFIDENCE = 0.55
_SCORE_LEVELS = (
    "Not a useful match for the requested guitar tone.",
    "A weak match; its catalog metadata has little overlap with the request.",
    "A plausible match with some relevant metadata.",
    "A strong match based on the available catalog metadata.",
    "An excellent metadata match for the requested tone.",
)


class JevError(RuntimeError):
    """A safe error from the optional TypeSafe decision service."""


class JevTransport(Protocol):
    def send(self, request: urllib.request.Request) -> dict[str, Any]: ...


class UrllibJevTransport:
    """Minimal transport: the catalog daemon needs no third-party SDK."""

    def send(self, request: urllib.request.Request) -> dict[str, Any]:
        try:
            with urllib.request.urlopen(request, timeout=6) as response:
                payload = json.loads(response.read())
        except urllib.error.HTTPError as error:
            raise JevError(f"TypeSafe request failed ({error.code})") from error
        except (urllib.error.URLError, TimeoutError) as error:
            raise JevError("TypeSafe could not be reached") from error
        if not isinstance(payload, dict):
            raise JevError("TypeSafe returned an invalid response")
        return payload


@dataclass(frozen=True)
class JevRigMatch:
    """A locally sortable match; no claim is made about the NAM audio itself."""

    pedal: dict[str, Any] | None
    amp: dict[str, Any]
    score: float
    confidence: float


class JevRigRanker:
    """Scores every bounded pedal/amp candidate pair in one Jev request."""

    def __init__(
        self,
        api_key: str,
        *,
        model: str = DEFAULT_JEV_MODEL,
        transport: JevTransport | None = None,
        min_confidence: float = DEFAULT_MIN_CONFIDENCE,
    ) -> None:
        self.api_key = api_key
        self.model = model
        self.transport = transport or UrllibJevTransport()
        self.min_confidence = min_confidence

    @property
    def available(self) -> bool:
        return bool(self.api_key.strip())

    def rank(
        self,
        plan: Mapping[str, Any],
        pedal_candidates: Sequence[dict[str, Any]],
        amp_candidates: Sequence[dict[str, Any]],
    ) -> list[JevRigMatch] | None:
        """Return the top three matches, or ``None`` when Jev is uncertain.

        The selector retains at most five candidates per role after diverse
        searches, leaving at most 25 independent Score questions in one
        bounded request. Ordinary deterministic code handles final ordering.
        """

        if not self.available:
            return None
        pairs = _candidate_pairs(pedal_candidates, amp_candidates)
        if not pairs:
            return []
        state = {
            "tone_plan": dict(plan),
            "candidate_rigs": [
                {
                    "index": index,
                    "pedal": _candidate_brief(pedal),
                    "amp": _candidate_brief(amp),
                }
                for index, (pedal, amp) in enumerate(pairs)
            ],
            "important_limit": (
                "This is metadata-only ranking. Do not infer unprovided audio "
                "behavior or claim an exact recording match."
            ),
        }
        questions = {
            f"rig_{index}": {
                "type": "score",
                "instructions": (
                    f"Rate candidate rig {index}'s likely suitability for the "
                    "tone plan. Judge only the supplied plan and catalog metadata."
                ),
                "criteria": list(_SCORE_LEVELS),
            }
            for index in range(len(pairs))
        }
        response = self._request(state, questions)
        matches = [
            JevRigMatch(
                pedal=pedal,
                amp=amp,
                score=_answer_number(response, f"rig_{index}", "score", 0.0, 4.0),
                confidence=_answer_number(
                    response, f"rig_{index}", "confidence", 0.0, 1.0
                ),
            )
            for index, (pedal, amp) in enumerate(pairs)
        ]
        matches.sort(
            key=lambda match: (match.score, match.confidence), reverse=True
        )
        if not matches or matches[0].confidence < self.min_confidence:
            return None
        return matches[:3]

    def _request(
        self, state: dict[str, Any], questions: dict[str, dict[str, Any]]
    ) -> dict[str, Any]:
        request = urllib.request.Request(
            TYPESAFE_SYSTEMONE_URL,
            data=json.dumps(
                {
                    "model": self.model,
                    "state": json.dumps(state, separators=(",", ":")),
                    "questions": questions,
                },
                separators=(",", ":"),
            ).encode(),
            headers={
                "Authorization": f"Bearer {self.api_key}",
                "Content-Type": "application/json",
            },
            method="POST",
        )
        return self.transport.send(request)


def _candidate_pairs(
    pedal_candidates: Sequence[dict[str, Any]], amp_candidates: Sequence[dict[str, Any]]
) -> list[tuple[dict[str, Any] | None, dict[str, Any]]]:
    if not amp_candidates:
        return []
    if not pedal_candidates:
        return [(None, amp) for amp in amp_candidates]
    return [
        (pedal, amp) for pedal in pedal_candidates for amp in amp_candidates
    ]


def _candidate_brief(candidate: dict[str, Any] | None) -> dict[str, str] | None:
    if candidate is None:
        return None
    return {
        "id": str(candidate.get("id", "")),
        "name": str(candidate.get("name", ""))[:160],
        "author": str(candidate.get("author", ""))[:100],
        "gear": str(candidate.get("gear", ""))[:100],
        "search_context": str(candidate.get("search_context", ""))[:100],
    }


def _answer_number(
    response: dict[str, Any],
    question_id: str,
    field: str,
    minimum: float,
    maximum: float,
) -> float:
    answers = response.get("answers")
    answer = answers.get(question_id) if isinstance(answers, dict) else None
    value = answer.get(field) if isinstance(answer, dict) else None
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        raise JevError(f"TypeSafe returned an invalid {field} for {question_id}")
    number = float(value)
    if not minimum <= number <= maximum:
        raise JevError(f"TypeSafe returned an out-of-range {field} for {question_id}")
    return number
