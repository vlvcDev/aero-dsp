"""Device-side AI planning for intentional, user-approved tone changes."""

from __future__ import annotations

import json
import secrets
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol

from .index import upsert_model
from .jev_ranker import JevError, JevRigRanker
from .tone3000_service import Tone3000Service
from .tone_memory import ToneMemoryError, ToneMemoryService


OPENAI_RESPONSES_URL = "https://api.openai.com/v1/responses"
PLAN_TTL_SECONDS = 10 * 60
MAX_AI_SEARCH_QUERIES = 4
MAX_AI_CANDIDATES_PER_ROLE = 5

CONTROL_RANGES = {
    "pedal_drive_db": (0.0, 24.0),
    "pedal_mix": (0.0, 1.0),
    "pedal_level_db": (-24.0, 12.0),
    "gate_threshold_db": (-80.0, -10.0),
    "compressor_ratio": (1.0, 20.0),
    "amp_bass_db": (-12.0, 12.0),
    "amp_mid_db": (-12.0, 12.0),
    "amp_treble_db": (-12.0, 12.0),
    "amp_volume_db": (-24.0, 12.0),
    "eq_low_db": (-12.0, 12.0),
    "eq_mid_db": (-12.0, 12.0),
    "eq_high_db": (-12.0, 12.0),
    "delay_mix": (0.0, 1.0),
    "reverb_mix": (0.0, 1.0),
}
EFFECT_KEYS = ("eq", "chorus", "delay", "reverb")
INTENT_VALUES = {
    "gain": ("clean", "edge", "crunch", "high"),
    "tightness": ("soft", "balanced", "tight"),
    "brightness": ("warm", "balanced", "bright"),
    "space": ("dry", "roomy", "ambient"),
    "boost": ("none", "light", "strong"),
}


class ToneMakerError(RuntimeError):
    """Raised for safe, user-facing AI tone-maker failures."""


class OpenAITransport(Protocol):
    def send(self, request: urllib.request.Request) -> dict[str, Any]: ...


class UrllibOpenAITransport:
    """Minimal dependency-free transport so the catalog stays easy to deploy."""

    def send(self, request: urllib.request.Request) -> dict[str, Any]:
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                payload = json.loads(response.read())
        except urllib.error.HTTPError as error:
            detail = ""
            try:
                body = json.loads(error.read())
                detail = str(body.get("error", {}).get("message") or body.get("message") or "")
            except (json.JSONDecodeError, UnicodeDecodeError, AttributeError):
                pass
            suffix = f": {detail}" if detail else ""
            raise ToneMakerError(f"OpenAI request failed ({error.code}){suffix}") from error
        except (urllib.error.URLError, TimeoutError) as error:
            raise ToneMakerError("OpenAI could not be reached") from error
        if not isinstance(payload, dict):
            raise ToneMakerError("OpenAI returned an invalid response")
        return payload


@dataclass(frozen=True)
class ToneIntent:
    """Musical direction, deliberately separate from raw DSP parameter values."""

    gain: str = "crunch"
    tightness: str = "balanced"
    brightness: str = "balanced"
    space: str = "dry"
    boost: str = "light"

    @classmethod
    def from_payload(cls, payload: Any) -> "ToneIntent":
        if payload is None:
            return cls()
        if not isinstance(payload, dict):
            raise ToneMakerError("OpenAI returned an invalid musical intent")
        values = {}
        for field, allowed in INTENT_VALUES.items():
            value = payload.get(field)
            if value not in allowed:
                raise ToneMakerError("OpenAI returned an invalid musical intent")
            values[field] = value
        return cls(**values)

    def as_payload(self) -> dict[str, str]:
        return {
            "gain": self.gain,
            "tightness": self.tightness,
            "brightness": self.brightness,
            "space": self.space,
            "boost": self.boost,
        }


def conservative_recipe(intent: ToneIntent) -> tuple[dict[str, float], dict[str, bool]]:
    """Turn musical direction into tame, explainable starting settings.

    NAM captures have no universal physical knob mapping. The recipe therefore
    only makes small post-capture corrections and never asks an unknown NAM to
    absorb extreme input gain. Per-model calibration will refine this later.
    """

    controls = {
        "pedal_drive_db": 0.0,
        "pedal_mix": 1.0,
        "pedal_level_db": 0.0,
        "gate_threshold_db": -80.0,
        "compressor_ratio": 2.5,
        "amp_bass_db": 0.0,
        "amp_mid_db": 0.0,
        "amp_treble_db": 0.0,
        "amp_volume_db": 0.0,
        "eq_low_db": 0.0,
        "eq_mid_db": 0.0,
        "eq_high_db": 0.0,
        "delay_mix": 0.0,
        "reverb_mix": 0.0,
    }
    effects = {name: False for name in EFFECT_KEYS}

    if intent.gain == "edge":
        controls["pedal_drive_db"] = 1.5
    elif intent.gain == "crunch":
        controls["pedal_drive_db"] = 3.0
    elif intent.gain == "high":
        controls["pedal_drive_db"] = 4.5
        controls["gate_threshold_db"] = -48.0

    if intent.boost == "light":
        controls["pedal_level_db"] = 1.5
    elif intent.boost == "strong":
        controls["pedal_drive_db"] = min(8.0, controls["pedal_drive_db"] + 2.0)
        controls["pedal_level_db"] = 3.0

    if intent.tightness == "tight":
        controls["amp_bass_db"] = -2.0
        controls["amp_mid_db"] = 1.0
        controls["eq_low_db"] = -1.5
        effects["eq"] = True
    elif intent.tightness == "soft":
        controls["amp_bass_db"] = 1.0
        controls["amp_mid_db"] = -0.5

    if intent.brightness == "warm":
        controls["amp_treble_db"] = -1.5
        controls["eq_high_db"] = -1.0
        effects["eq"] = True
    elif intent.brightness == "bright":
        controls["amp_treble_db"] = 1.0
        controls["eq_high_db"] = 0.75
        effects["eq"] = True

    if intent.space == "roomy":
        controls["reverb_mix"] = 0.12
        effects["reverb"] = True
    elif intent.space == "ambient":
        controls["delay_mix"] = 0.16
        controls["reverb_mix"] = 0.22
        effects["delay"] = True
        effects["reverb"] = True
    return controls, effects


@dataclass(frozen=True)
class TonePlan:
    summary: str
    pedal_query: str
    amp_query: str
    intent: ToneIntent
    gear_hints: tuple[str, ...]
    controls: dict[str, float]
    effects: dict[str, bool]

    @classmethod
    def from_payload(cls, payload: dict[str, Any]) -> "TonePlan":
        try:
            summary = str(payload["summary"]).strip()[:240]
            pedal_query = str(payload["pedal_query"]).strip()[:160]
            amp_query = str(payload["amp_query"]).strip()[:160]
        except (KeyError, TypeError, ValueError) as error:
            raise ToneMakerError("OpenAI returned an incomplete tone plan") from error
        if not summary or not pedal_query or not amp_query:
            raise ToneMakerError("OpenAI returned an incomplete tone plan")
        intent = ToneIntent.from_payload(payload.get("intent"))
        raw_hints = payload.get("gear_hints", [])
        if not isinstance(raw_hints, list) or len(raw_hints) > 4:
            raise ToneMakerError("OpenAI returned invalid gear search hints")
        gear_hints = tuple(
            hint.strip()[:80]
            for hint in raw_hints
            if isinstance(hint, str) and hint.strip()
        )
        # Old local Tone Memory records include controls directly. Keep them
        # usable, but new cloud plans always derive values from ToneIntent.
        if "controls" in payload and "intent" not in payload:
            try:
                controls = {
                    name: float(payload["controls"][name]) for name in CONTROL_RANGES
                }
                effects = {name: bool(payload["effects"][name]) for name in EFFECT_KEYS}
            except (KeyError, TypeError, ValueError) as error:
                raise ToneMakerError("A saved tone recipe is incomplete") from error
        else:
            controls, effects = conservative_recipe(intent)
        for name, value in controls.items():
            low, high = CONTROL_RANGES[name]
            if not low <= value <= high:
                raise ToneMakerError("Tone controls are outside their safe range")
        return cls(summary, pedal_query, amp_query, intent, gear_hints, controls, effects)

    def as_payload(self) -> dict[str, Any]:
        return {
            "summary": self.summary,
            "pedal_query": self.pedal_query,
            "amp_query": self.amp_query,
            "intent": self.intent.as_payload(),
            "gear_hints": list(self.gear_hints),
            "controls": self.controls,
            "effects": self.effects,
        }


@dataclass(frozen=True)
class ToneRig:
    """One explicitly selectable pedal, amp, and compatible cabinet choice."""

    label: str
    rationale: str
    pedal: dict[str, Any] | None
    amp: dict[str, Any]
    cabinet: dict[str, Any] | None = None
    cabinet_mode: str = "manual"
    ranking_confidence: float | None = None
    ranking_score: float | None = None

    def as_payload(self) -> dict[str, Any]:
        return {
            "label": self.label,
            "rationale": self.rationale,
            "pedal": self.pedal,
            "amp": self.amp,
            "cabinet": self.cabinet,
            "cabinet_mode": self.cabinet_mode,
            "ranking_confidence": self.ranking_confidence,
            "ranking_score": self.ranking_score,
        }


@dataclass(frozen=True)
class ToneRecommendation:
    identifier: str
    expires_at: float
    prompt: str
    plan: TonePlan
    rigs: list[ToneRig]
    ranking_provider: str
    ranking_note: str | None = None

    def as_payload(self) -> dict[str, Any]:
        return {
            "recommendation_id": self.identifier,
            "expires_at": self.expires_at,
            "plan": self.plan.as_payload(),
            "rigs": [rig.as_payload() for rig in self.rigs],
            "ranking_provider": self.ranking_provider,
            "ranking_note": self.ranking_note,
        }


class ToneMakerService:
    """Plans with OpenAI, searches TONE3000, and optionally ranks with Jev."""

    def __init__(
        self,
        api_key: str,
        *,
        model: str = "gpt-5-mini",
        transport: OpenAITransport | None = None,
        jev_ranker: JevRigRanker | None = None,
        tone_memory: ToneMemoryService | None = None,
        clock: callable = time.time,
    ) -> None:
        self.api_key = api_key
        self.model = model
        self.transport = transport or UrllibOpenAITransport()
        self.jev_ranker = jev_ranker
        self.tone_memory = tone_memory
        self.clock = clock
        self._recommendations: dict[str, ToneRecommendation] = {}

    @property
    def available(self) -> bool:
        return bool(self.api_key.strip())

    def status(self, tone3000: Tone3000Service | None) -> dict[str, Any]:
        try:
            memory_available = bool(self.tone_memory and self.tone_memory.has_records())
        except ToneMemoryError:
            memory_available = False
        return {
            "available": self.available,
            "tone3000_connected": bool(tone3000 and tone3000.connected),
            "jev_available": bool(self.jev_ranker and self.jev_ranker.available),
            "tone_memory_available": memory_available,
        }

    def recommend(
        self, prompt: str, tone3000: Tone3000Service | None
    ) -> ToneRecommendation:
        prompt = prompt.strip()
        if not 3 <= len(prompt) <= 500:
            raise ToneMakerError("Describe the tone in 3 to 500 characters")
        if self.tone_memory is not None:
            # Each recipe carries its own controls/effects; recall one proven
            # match at a time rather than mixing a different rig with the
            # first match's settings.
            try:
                saved = self.tone_memory.matches(prompt, limit=1)
            except ToneMemoryError as error:
                raise ToneMakerError(str(error)) from error
            if saved:
                return self._recommend_from_memory(prompt, saved)
        if not self.available:
            raise ToneMakerError("OpenAI is not configured on this device")
        if tone3000 is None or not tone3000.connected:
            raise ToneMakerError("Connect your TONE3000 account before making a tone")
        plan = self._plan(prompt)
        pedal_candidates = _search_candidates(tone3000, plan, role="pedal")
        amp_candidates = _search_candidates(tone3000, plan, role="amp")
        if not amp_candidates:
            raise ToneMakerError("TONE3000 did not return compatible NAM A2 candidates")
        ranking_provider, ranking_note, rigs = self._rank_rigs(
            plan, pedal_candidates, amp_candidates
        )
        rigs = self._attach_cabinets(tone3000, plan, rigs)
        self._prune_expired()
        recommendation = ToneRecommendation(
            identifier=secrets.token_urlsafe(18),
            expires_at=self.clock() + PLAN_TTL_SECONDS,
            prompt=prompt,
            plan=plan,
            rigs=rigs,
            ranking_provider=ranking_provider,
            ranking_note=ranking_note,
        )
        self._recommendations[recommendation.identifier] = recommendation
        return recommendation

    def apply(
        self,
        recommendation_id: str,
        rig_index: int,
        tone3000: Tone3000Service | None,
        models_dir: Path,
        database: Path,
    ) -> dict[str, Any]:
        self._prune_expired()
        recommendation = self._recommendations.get(recommendation_id)
        if recommendation is None:
            raise ToneMakerError("That AI recommendation expired; ask again to refresh it")
        if not 0 <= rig_index < len(recommendation.rigs):
            raise ToneMakerError("That rig choice is no longer available; ask again to refresh it")
        rig = recommendation.rigs[rig_index]
        needs_download = any(
            candidate is not None and "local_path" not in candidate
            for candidate in (rig.pedal, rig.amp, rig.cabinet)
        )
        if needs_download and (tone3000 is None or not tone3000.connected):
            raise ToneMakerError("Connect your TONE3000 account before applying a tone")
        installed: dict[str, Any] = {}
        for role, candidate in (
            ("pre_model", rig.pedal),
            ("amp_model", rig.amp),
        ):
            if candidate is None:
                continue
            local_path = candidate.get("local_path")
            if isinstance(local_path, str):
                path = Path(local_path)
                if not path.is_file() or path.suffix != ".nam":
                    raise ToneMakerError("A saved Tone Memory model is no longer available")
                source = str(candidate.get("source", "tone-memory"))
            else:
                tone_id = str(candidate["id"])
                assert tone3000 is not None
                path, _ = tone3000.download_best_model(tone_id, models_dir)
                source = f"tone3000:ai:{tone_id}"
            installed[role] = {
                **upsert_model(path, database, source=source).__dict__,
                "tone": candidate,
            }
        if "amp_model" not in installed:
            raise ToneMakerError("No compatible amp NAM was available to apply")
        cabinet_ir: dict[str, str] | None = None
        if rig.cabinet is not None:
            candidate = rig.cabinet
            local_path = candidate.get("local_path")
            if isinstance(local_path, str):
                path = Path(local_path)
                if not path.is_file() or path.suffix.casefold() != ".wav":
                    raise ToneMakerError("A saved cabinet IR is no longer available")
                source = str(candidate.get("source", "tone-memory"))
            else:
                tone_id = str(candidate["id"])
                assert tone3000 is not None
                path, _ = tone3000.download_best_ir(tone_id, models_dir.parent / "irs")
                source = f"tone3000:ai-ir:{tone_id}"
                upsert_model(path, database, source=source)
            cabinet_ir = {"name": path.stem, "path": str(path), "source": source}
        self._recommendations.pop(recommendation_id, None)
        tone_memory = {
            "query": recommendation.prompt,
            "summary": recommendation.plan.summary,
            "rig_label": rig.label,
            "controls": recommendation.plan.controls,
            "effects": recommendation.plan.effects,
            "amp_model": _memory_model(installed["amp_model"]),
        }
        if "pre_model" in installed:
            tone_memory["pre_model"] = _memory_model(installed["pre_model"])
        if cabinet_ir is not None:
            tone_memory["cabinet_ir"] = cabinet_ir
        return {
            "summary": recommendation.plan.summary,
            "rig_label": rig.label,
            "controls": recommendation.plan.controls,
            "effects": recommendation.plan.effects,
            "tone_memory": tone_memory,
            "cabinet_ir": cabinet_ir,
            "cabinet_mode": rig.cabinet_mode,
            **installed,
        }

    def _plan(self, prompt: str) -> TonePlan:
        return TonePlan.from_payload(
            self._request_structured(
                name="aero_dsp_tone_plan",
                schema=_PLAN_SCHEMA,
                system_prompt=_SYSTEM_PROMPT,
                user_input=prompt,
            )
        )

    def _attach_cabinets(
        self,
        tone3000: Tone3000Service,
        plan: TonePlan,
        rigs: list[ToneRig],
    ) -> list[ToneRig]:
        """Choose an IR only for verified head-only NAM captures.

        TONE3000's gear classification—not model-name guesswork—decides
        whether an IR is allowed. This prevents a second cabinet stage after
        an Amp + Cab NAM capture.
        """

        head_rigs = [rig for rig in rigs if _is_amp_head(rig.amp)]
        cabinet_candidates = (
            _search_candidates(tone3000, plan, role="cabinet") if head_rigs else []
        )
        enriched: list[ToneRig] = []
        for rig in rigs:
            if _has_embedded_cab(rig.amp):
                cabinet = None
                mode = "embedded"
            elif _is_amp_head(rig.amp):
                cabinet = _best_cabinet_for_amp(cabinet_candidates, rig.amp)
                mode = "selected" if cabinet is not None else "not_found"
            else:
                cabinet = None
                mode = "manual"
            enriched.append(
                ToneRig(
                    label=rig.label,
                    rationale=rig.rationale,
                    pedal=rig.pedal,
                    amp=rig.amp,
                    cabinet=cabinet,
                    cabinet_mode=mode,
                    ranking_confidence=rig.ranking_confidence,
                    ranking_score=rig.ranking_score,
                )
            )
        return enriched


    def _recommend_from_memory(
        self, prompt: str, records: list[dict[str, Any]]
    ) -> ToneRecommendation:
        first = records[0]
        plan = TonePlan.from_payload(
            {
                "summary": first["summary"],
                "pedal_query": "saved personal pedal",
                "amp_query": "saved personal amp",
                "controls": first["controls"],
                "effects": first["effects"],
            }
        )
        rigs = [
            ToneRig(
                label=f"Your saved tone: {record['rig_label']}",
                rationale=(
                    f"You confirmed this for “{record['query']}”. "
                    "It uses cached NAMs and works offline."
                ),
                pedal=_local_candidate(record.get("pre_model"), "pedal", record["id"]),
                amp=_local_candidate(record["amp_model"], "amp", record["id"]),
                cabinet=_local_candidate(
                    record.get("cabinet_ir"), "cabinet", record["id"]
                ),
                cabinet_mode="selected" if record.get("cabinet_ir") else "manual",
            )
            for record in records
        ]
        recommendation = ToneRecommendation(
            identifier=secrets.token_urlsafe(18),
            expires_at=self.clock() + PLAN_TTL_SECONDS,
            prompt=prompt,
            plan=plan,
            rigs=rigs,
            ranking_provider="tone_memory",
            ranking_note="Your locally confirmed tone matches this request.",
        )
        self._prune_expired()
        self._recommendations[recommendation.identifier] = recommendation
        return recommendation

    def _rank_rigs(
        self,
        plan: TonePlan,
        pedal_candidates: list[dict[str, Any]],
        amp_candidates: list[dict[str, Any]],
    ) -> tuple[str, str | None, list[ToneRig]]:
        """Prefer Jev for bounded metadata ranking; retain OpenAI as fallback."""

        if self.jev_ranker and self.jev_ranker.available:
            try:
                matches = self.jev_ranker.rank(
                    plan.as_payload(), pedal_candidates, amp_candidates
                )
            except JevError as error:
                return (
                    "openai",
                    str(error),
                    self._rank_rigs_with_openai(
                        plan, pedal_candidates, amp_candidates
                    ),
                )
            if matches:
                return (
                    "jev",
                    None,
                    [
                        ToneRig(
                            label=f"Jev pick {index + 1}",
                            rationale=_jev_rationale(match.score, match.confidence),
                            pedal=match.pedal,
                            amp=match.amp,
                            ranking_confidence=match.confidence,
                            ranking_score=match.score / 4.0,
                        )
                        for index, match in enumerate(matches)
                    ],
                )
            return (
                "openai",
                "Jev was not confident enough to rank these catalog matches.",
                self._rank_rigs_with_openai(
                    plan, pedal_candidates, amp_candidates
                ),
            )
        return (
            "openai",
            "Jev is not configured on this device.",
            self._rank_rigs_with_openai(
                plan, pedal_candidates, amp_candidates
            ),
        )

    def _rank_rigs_with_openai(
        self,
        plan: TonePlan,
        pedal_candidates: list[dict[str, Any]],
        amp_candidates: list[dict[str, Any]],
    ) -> list[ToneRig]:
        count = min(3, len(pedal_candidates), len(amp_candidates))
        if count == 0:
            return [
                ToneRig(
                    label=f"Amp pick {index + 1}",
                    rationale="No compatible drive NAM was found, so this rig uses the amp alone.",
                    pedal=None,
                    amp=amp,
                )
                for index, amp in enumerate(amp_candidates[:3])
            ]
        candidate_payload = {
            "plan": plan.as_payload(),
            "candidate_count": count,
            "pedal_candidates": [_candidate_brief(candidate) for candidate in pedal_candidates],
            "amp_candidates": [_candidate_brief(candidate) for candidate in amp_candidates],
        }
        ranked = self._request_structured(
                name="aero_dsp_rig_ranking",
            schema=_rig_schema(count),
            system_prompt=_RANKING_PROMPT,
            user_input=json.dumps(candidate_payload, separators=(",", ":")),
        )
        raw_rigs = ranked.get("rigs")
        if not isinstance(raw_rigs, list) or len(raw_rigs) != count:
            raise ToneMakerError("OpenAI returned an incomplete rig ranking")
        pedals_by_id = {str(candidate["id"]): candidate for candidate in pedal_candidates}
        amps_by_id = {str(candidate["id"]): candidate for candidate in amp_candidates}
        rigs: list[ToneRig] = []
        pairs: set[tuple[str, str]] = set()
        for raw_rig in raw_rigs:
            if not isinstance(raw_rig, dict):
                raise ToneMakerError("OpenAI returned an invalid rig ranking")
            pedal_id = str(raw_rig.get("pedal_tone_id", ""))
            amp_id = str(raw_rig.get("amp_tone_id", ""))
            pedal = pedals_by_id.get(pedal_id)
            amp = amps_by_id.get(amp_id)
            if pedal is None or amp is None or (pedal_id, amp_id) in pairs:
                raise ToneMakerError("OpenAI chose a rig outside the available catalog results")
            pairs.add((pedal_id, amp_id))
            label = str(raw_rig.get("label", "")).strip()[:48]
            rationale = str(raw_rig.get("rationale", "")).strip()[:180]
            if not label or not rationale:
                raise ToneMakerError("OpenAI returned an incomplete rig ranking")
            rigs.append(ToneRig(label, rationale, pedal, amp))
        return rigs

    def _request_structured(
        self,
        *,
        name: str,
        schema: dict[str, Any],
        system_prompt: str,
        user_input: str,
    ) -> dict[str, Any]:
        request = urllib.request.Request(
            OPENAI_RESPONSES_URL,
            data=json.dumps(
                {
                    "model": self.model,
                    "store": False,
                    "input": [
                        {"role": "system", "content": system_prompt},
                        {"role": "user", "content": user_input},
                    ],
                    "text": {
                        "format": {
                            "type": "json_schema",
                            "name": name,
                            "strict": True,
                            "schema": schema,
                        }
                    },
                },
                separators=(",", ":"),
            ).encode(),
            headers={
                "Authorization": f"Bearer {self.api_key}",
                "Content-Type": "application/json",
            },
            method="POST",
        )
        response = self.transport.send(request)
        return _structured_output(response)

    def _prune_expired(self) -> None:
        now = self.clock()
        self._recommendations = {
            identifier: recommendation
            for identifier, recommendation in self._recommendations.items()
            if recommendation.expires_at > now
        }


def _search_queries(plan: TonePlan, role: str) -> tuple[str, ...]:
    """Build a deliberately small, diverse search set from the plan.

    Gear hints are discovery leads rather than asserted artist facts. A later
    evidence provider can supply them from cited sources; this stage already
    benefits from safe, model-provided hardware aliases without maintaining a
    static artist database.
    """

    if role == "cabinet":
        # Cabinet packs are named for speakers, microphones, and dimensions
        # much more often than for the amp head they pair with. Always retain
        # a broad cabinet query rather than letting artist/amp hints crowd it
        # out of the bounded search budget.
        return (f"{plan.amp_query} cab", "cabinet", "4x12", "guitar cab")

    primary = plan.pedal_query if role == "pedal" else plan.amp_query
    if role == "pedal" and plan.intent.tightness == "tight":
        fallback = "transparent overdrive boost"
    elif role == "pedal":
        fallback = "overdrive pedal"
    elif plan.intent.gain == "high":
        fallback = "high gain amp"
    else:
        fallback = "guitar amp"
    queries = [primary, *plan.gear_hints, fallback]
    unique: list[str] = []
    seen: set[str] = set()
    for query in queries:
        normalized = " ".join(query.casefold().split())
        if normalized and normalized not in seen:
            seen.add(normalized)
            unique.append(query)
        if len(unique) == MAX_AI_SEARCH_QUERIES:
            break
    return tuple(unique)


def _search_candidates(
    tone3000: Tone3000Service, plan: TonePlan, *, role: str
) -> list[dict[str, Any]]:
    """Round-robin query results so one broad search cannot crowd out gear hits."""

    queries = _search_queries(plan, role)
    searches = [tone3000.search(query, role=role) for query in queries]
    results: list[dict[str, Any]] = []
    seen: set[str] = set()
    for candidate_index in range(max((len(items) for items in searches), default=0)):
        for query, items in zip(queries, searches):
            if candidate_index >= len(items):
                continue
            candidate = dict(items[candidate_index])
            identifier = str(candidate.get("id", ""))
            if not identifier or identifier in seen:
                continue
            seen.add(identifier)
            candidate["search_context"] = query[:160]
            results.append(candidate)
            if len(results) == MAX_AI_CANDIDATES_PER_ROLE:
                return results
    return results


def _has_embedded_cab(candidate: dict[str, Any]) -> bool:
    gear = str(candidate.get("gear", "")).casefold().replace("_", "-")
    return "amp-cab" in gear or "amp + cab" in gear or "amp and cab" in gear


def _is_amp_head(candidate: dict[str, Any]) -> bool:
    """Return true only for an explicit head-only TONE3000 gear category."""

    gear = str(candidate.get("gear", "")).casefold().replace("_", "-")
    if _has_embedded_cab(candidate):
        return False
    return gear in {"amp", "amp head", "amp-head"}


def _best_cabinet_for_amp(
    cabinets: list[dict[str, Any]], amp: dict[str, Any]
) -> dict[str, Any] | None:
    if not cabinets:
        return None
    amp_words = {
        word
        for word in str(amp.get("name", "")).casefold().replace("-", " ").split()
        if len(word) >= 3
    }

    def score(cabinet: dict[str, Any]) -> tuple[int, int]:
        text = " ".join(
            str(cabinet.get(key, "")) for key in ("name", "search_context")
        ).casefold()
        matches = sum(word in text for word in amp_words)
        # The API's existing sort remains the stable tie-breaker; matching
        # just lets a specific 4x12/Mesa/Marshall search win when available.
        return matches, -cabinets.index(cabinet)

    return max(cabinets, key=score)


def _structured_output(response: dict[str, Any]) -> dict[str, Any]:
    text = response.get("output_text")
    if not isinstance(text, str):
        for item in response.get("output", []):
            if not isinstance(item, dict):
                continue
            for content in item.get("content", []):
                if isinstance(content, dict) and isinstance(content.get("text"), str):
                    text = content["text"]
                    break
            if isinstance(text, str):
                break
    if not isinstance(text, str):
        raise ToneMakerError("OpenAI did not return a tone plan")
    try:
        payload = json.loads(text)
    except json.JSONDecodeError as error:
        raise ToneMakerError("OpenAI returned an invalid tone plan") from error
    if not isinstance(payload, dict):
        raise ToneMakerError("OpenAI returned an invalid tone plan")
    return payload


_SYSTEM_PROMPT = """You are AERO>>DSP's guitar tone planner. Convert a player's
description into practical searches for a pedal NAM and an amp NAM plus a
musical intent. Do not output DSP knob positions: AERO>>DSP derives conservative
settings deterministically after the plan. Do not claim a perfect replica of an
artist or recording. Gear hints are merely compact catalog-search leads, never
asserted historical facts. Favor tight, musical, playable settings. Search
terms must be short gear- or genre-oriented phrases likely to occur in a NAM
catalog. Return only the required JSON schema."""

_RANKING_PROMPT = """You are AERO>>DSP's guitar rig curator. Rank the requested
number of distinct pedal-NAM plus amp-NAM pairs from the supplied candidate lists
for the provided tone plan. Use only IDs that appear in those lists. Give each
pair a short, useful label and a plain-language reason it fits the plan. Do not
claim an exact replica of any artist or recording. Return only the required JSON
schema."""


def _candidate_brief(candidate: dict[str, Any]) -> dict[str, str]:
    """Give the ranker only catalog metadata, never model content or credentials."""

    return {
        "id": str(candidate.get("id", "")),
        "name": str(candidate.get("name", ""))[:160],
        "author": str(candidate.get("author", ""))[:100],
        "gear": str(candidate.get("gear", ""))[:100],
        "search_context": str(candidate.get("search_context", ""))[:100],
    }


def _jev_rationale(score: float, confidence: float) -> str:
    match = (
        "excellent"
        if score >= 3.5
        else "strong"
        if score >= 2.5
        else "plausible"
        if score >= 1.5
        else "weak"
    )
    return (
        f"Jev rates this as a {match} catalog-metadata match "
        f"({round(confidence * 100)}% confidence). Compare it by ear before applying."
    )


def _memory_model(installed: dict[str, Any]) -> dict[str, str]:
    """Keep only the portable model identity needed to recall a local rig."""
    return {
        key: str(installed.get(key, ""))
        for key in ("name", "path", "source")
    }


def _local_candidate(
    model: Any, gear: str, record_id: str
) -> dict[str, Any] | None:
    if model is None:
        return None
    if not isinstance(model, dict):
        raise ToneMakerError("A saved Tone Memory recipe is invalid")
    path = model.get("path")
    name = model.get("name")
    source = model.get("source")
    if not all(isinstance(value, str) and value for value in (path, name, source)):
        raise ToneMakerError("A saved Tone Memory recipe is invalid")
    return {
        "id": f"memory:{record_id}:{gear}",
        "name": name,
        "author": "Your Tone Memory",
        "gear": gear,
        "local_path": path,
        "source": source,
    }


def _rig_schema(count: int) -> dict[str, Any]:
    return {
        "type": "object",
        "additionalProperties": False,
        "required": ["rigs"],
        "properties": {
            "rigs": {
                "type": "array",
                "minItems": count,
                "maxItems": count,
                "items": {
                    "type": "object",
                    "additionalProperties": False,
                    "required": [
                        "pedal_tone_id",
                        "amp_tone_id",
                        "label",
                        "rationale",
                    ],
                    "properties": {
                        "pedal_tone_id": {"type": "string"},
                        "amp_tone_id": {"type": "string"},
                        "label": {"type": "string", "maxLength": 48},
                        "rationale": {"type": "string", "maxLength": 180},
                    },
                },
            },
        },
    }

_PLAN_SCHEMA: dict[str, Any] = {
    "type": "object",
    "additionalProperties": False,
    "required": ["summary", "pedal_query", "amp_query", "intent", "gear_hints"],
    "properties": {
        "summary": {"type": "string", "maxLength": 240},
        "pedal_query": {"type": "string", "maxLength": 160},
        "amp_query": {"type": "string", "maxLength": 160},
        "intent": {
            "type": "object",
            "additionalProperties": False,
            "required": list(INTENT_VALUES),
            "properties": {
                name: {"type": "string", "enum": list(allowed)}
                for name, allowed in INTENT_VALUES.items()
            },
        },
        "gear_hints": {
            "type": "array",
            "minItems": 0,
            "maxItems": 4,
            "items": {"type": "string", "maxLength": 80},
        },
    },
}
