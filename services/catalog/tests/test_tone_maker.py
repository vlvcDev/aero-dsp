import json
import sys
import tempfile
import unittest
from pathlib import Path

CATALOG_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(CATALOG_ROOT))

from pedal_catalog.tone_maker import (  # noqa: E402
    CONTROL_RANGES,
    ToneMakerService,
    TonePlan,
)
from pedal_catalog.jev_ranker import JevError, JevRigRanker  # noqa: E402
from pedal_catalog.tone_memory import ToneMemoryService  # noqa: E402


class FakeOpenAITransport:
    def __init__(self, *payloads):
        self.payloads = list(payloads)
        self.requests = []

    def send(self, request):
        self.requests.append(request)
        return {
            "output": [
                {"content": [{"type": "output_text", "text": json.dumps(self.payloads.pop(0))}]}
            ]
        }


class FakeTone3000:
    connected = True

    def __init__(self, *, amp_gear="amp"):
        self.searches = []
        self.downloads = []
        self.amp_gear = amp_gear

    def search(self, query, *, role):
        self.searches.append((query, role))
        return [
            {
                "id": f"{role}-{index}",
                "name": f"{role.title()} Candidate {index}",
                "author": "Ada",
                "gear": self.amp_gear if role == "amp" else role,
            }
            for index in range(1, 4)
        ]

    def download_best_model(self, tone_id, models_dir):
        path = models_dir / f"{tone_id}.nam"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"validated nam")
        self.downloads.append(tone_id)
        return path, "hash"

    def download_best_ir(self, tone_id, irs_dir):
        path = irs_dir / f"{tone_id}.wav"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"validated wav")
        self.downloads.append(tone_id)
        return path, "hash"


class FakeJevTransport:
    def __init__(self, scores):
        self.scores = scores
        self.requests = []

    def send(self, request):
        self.requests.append(request)
        body = json.loads(request.data)
        return {
            "answers": {
                question_id: {
                    "type": "score",
                    "score": self.scores[question_id][0],
                    "confidence": self.scores[question_id][1],
                    "probabilities": {},
                }
                for question_id in body["questions"]
            }
        }


class FailingJevTransport:
    def send(self, request):
        raise JevError("TypeSafe request failed (401)")


def plan_payload():
    return {
        "summary": "Tight high-gain rhythm with a dry, focused finish.",
        "pedal_query": "tight overdrive pedal",
        "amp_query": "high gain metal amp",
        "controls": {name: (low + high) / 2 for name, (low, high) in CONTROL_RANGES.items()},
        "effects": {"eq": True, "chorus": False, "delay": False, "reverb": False},
    }


def ranking_payload():
    return {
        "rigs": [
            {
                "pedal_tone_id": "pedal-2",
                "amp_tone_id": "amp-3",
                "label": "Focused thrash",
                "rationale": "Tightens the low end into the most aggressive amp option.",
            },
            {
                "pedal_tone_id": "pedal-1",
                "amp_tone_id": "amp-1",
                "label": "Balanced high gain",
                "rationale": "A balanced fallback with clear note separation.",
            },
            {
                "pedal_tone_id": "pedal-3",
                "amp_tone_id": "amp-2",
                "label": "Wide rhythm",
                "rationale": "A broader alternative for heavier chord work.",
            },
        ]
    }


class ToneMakerTests(unittest.TestCase):
    def test_amp_plus_cab_candidate_never_searches_or_loads_a_second_ir(self):
        transport = FakeOpenAITransport(plan_payload(), ranking_payload())
        service = ToneMakerService("sk-test", transport=transport, clock=lambda: 100)
        tone3000 = FakeTone3000(amp_gear="amp-cab")

        recommendation = service.recommend("Give me a tight metal rhythm tone", tone3000)

        self.assertTrue(all(role != "cabinet" for _, role in tone3000.searches))
        self.assertEqual(recommendation.rigs[0].cabinet_mode, "embedded")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            applied = service.apply(
                recommendation.identifier,
                0,
                tone3000,
                root / "models",
                root / "catalog.db",
            )
        self.assertIsNone(applied["cabinet_ir"])
        self.assertEqual(tone3000.downloads, ["pedal-2", "amp-3"])

    def test_plan_ranks_three_candidate_rigs_then_applies_the_selected_one(self):
        transport = FakeOpenAITransport(plan_payload(), ranking_payload())
        service = ToneMakerService("sk-test", transport=transport, clock=lambda: 100)
        tone3000 = FakeTone3000()

        recommendation = service.recommend("Give me a tight metal rhythm tone", tone3000)

        body = json.loads(transport.requests[0].data)
        self.assertEqual(body["model"], "gpt-5-mini")
        self.assertFalse(body["store"])
        self.assertEqual(body["text"]["format"]["type"], "json_schema")
        ranking_body = json.loads(transport.requests[1].data)
        self.assertEqual(ranking_body["text"]["format"]["name"], "aero_dsp_rig_ranking")
        self.assertEqual(len(recommendation.rigs), 3)
        self.assertEqual(recommendation.ranking_provider, "openai")
        self.assertEqual(
            recommendation.ranking_note, "Jev is not configured on this device."
        )
        self.assertEqual(recommendation.rigs[0].label, "Focused thrash")
        self.assertEqual(
            tone3000.searches,
            [
                ("tight overdrive pedal", "pedal"),
                ("overdrive pedal", "pedal"),
                ("high gain metal amp", "amp"),
                ("guitar amp", "amp"),
                ("high gain metal amp cab", "cabinet"),
                ("cabinet", "cabinet"),
                ("4x12", "cabinet"),
                ("guitar cab", "cabinet"),
            ],
        )

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            applied = service.apply(
                recommendation.identifier,
                0,
                tone3000,
                root / "models",
                root / "catalog.db",
            )

        self.assertEqual(tone3000.downloads, ["pedal-2", "amp-3", "cabinet-1"])
        self.assertEqual(applied["pre_model"]["source"], "tone3000:ai:pedal-2")
        self.assertEqual(applied["amp_model"]["source"], "tone3000:ai:amp-3")
        self.assertEqual(applied["rig_label"], "Focused thrash")
        self.assertTrue(applied["effects"]["eq"])
        self.assertEqual(applied["tone_memory"]["query"], "Give me a tight metal rhythm tone")
        self.assertEqual(applied["tone_memory"]["amp_model"]["path"], str(root / "models" / "amp-3.nam"))
        self.assertEqual(applied["cabinet_ir"]["path"], str(root / "irs" / "cabinet-1.wav"))

    def test_jev_scores_each_candidate_pair_then_keeps_openai_for_planning(self):
        openai = FakeOpenAITransport(plan_payload())
        jev_transport = FakeJevTransport(
            {
                "rig_0": (1.0, 0.7),
                "rig_1": (2.0, 0.75),
                "rig_2": (1.5, 0.8),
                "rig_3": (3.0, 0.81),
                "rig_4": (4.0, 0.93),
                "rig_5": (2.5, 0.68),
                "rig_6": (2.0, 0.7),
                "rig_7": (3.5, 0.9),
                "rig_8": (2.5, 0.76),
            }
        )
        service = ToneMakerService(
            "sk-test",
            transport=openai,
            jev_ranker=JevRigRanker("ts-test", transport=jev_transport),
            clock=lambda: 100,
        )

        recommendation = service.recommend(
            "Give me a tight metal rhythm tone", FakeTone3000()
        )

        self.assertEqual(len(openai.requests), 1)
        self.assertEqual(recommendation.ranking_provider, "jev")
        self.assertEqual(recommendation.rigs[0].pedal["id"], "pedal-2")
        self.assertEqual(recommendation.rigs[0].amp["id"], "amp-2")
        self.assertEqual(recommendation.rigs[0].ranking_confidence, 0.93)
        self.assertEqual(recommendation.rigs[0].ranking_score, 1.0)
        self.assertIn("93% confidence", recommendation.rigs[0].rationale)

        request = jev_transport.requests[0]
        body = json.loads(request.data)
        self.assertEqual(request.full_url, "https://api.typesafe.ai/v1/systemone")
        self.assertEqual(body["model"], "jev-latest")
        self.assertEqual(len(body["questions"]), 9)
        self.assertEqual(request.get_header("Authorization"), "Bearer ts-test")

    def test_uncertain_jev_result_uses_the_existing_openai_ranker(self):
        openai = FakeOpenAITransport(plan_payload(), ranking_payload())
        jev_transport = FakeJevTransport(
            {f"rig_{index}": (3.0, 0.2) for index in range(9)}
        )
        service = ToneMakerService(
            "sk-test",
            transport=openai,
            jev_ranker=JevRigRanker("ts-test", transport=jev_transport),
            clock=lambda: 100,
        )

        recommendation = service.recommend(
            "Give me a tight metal rhythm tone", FakeTone3000()
        )

        self.assertEqual(recommendation.ranking_provider, "openai")
        self.assertEqual(recommendation.rigs[0].label, "Focused thrash")
        self.assertEqual(
            recommendation.ranking_note,
            "Jev was not confident enough to rank these catalog matches.",
        )
        self.assertEqual(len(openai.requests), 2)

    def test_failed_jev_request_keeps_the_openai_result_and_a_safe_reason(self):
        openai = FakeOpenAITransport(plan_payload(), ranking_payload())
        service = ToneMakerService(
            "sk-test",
            transport=openai,
            jev_ranker=JevRigRanker("ts-test", transport=FailingJevTransport()),
            clock=lambda: 100,
        )

        recommendation = service.recommend(
            "Give me a tight metal rhythm tone", FakeTone3000()
        )

        self.assertEqual(recommendation.ranking_provider, "openai")
        self.assertEqual(recommendation.ranking_note, "TypeSafe request failed (401)")
        self.assertEqual(recommendation.rigs[0].label, "Focused thrash")

    def test_confirmed_local_tone_is_recalled_before_any_network_request(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            amp = root / "amp.nam"
            amp.write_bytes(b"validated nam")
            memory = ToneMemoryService(root / "tone-memory.json")
            memory.save(
                {
                    "query": "tight metal rhythm",
                    "summary": plan_payload()["summary"],
                    "rig_label": "Focused thrash",
                    "amp_model": {
                        "name": "amp.nam",
                        "path": str(amp),
                        "source": "tone3000:ai:amp-3",
                    },
                    "controls": plan_payload()["controls"],
                    "effects": plan_payload()["effects"],
                }
            )
            transport = FakeOpenAITransport()
            service = ToneMakerService(
                "",
                transport=transport,
                tone_memory=memory,
                clock=lambda: 100,
            )

            recommendation = service.recommend("I need a tight metal tone", None)

        self.assertEqual(recommendation.ranking_provider, "tone_memory")
        self.assertEqual(recommendation.rigs[0].amp["local_path"], str(amp))
        self.assertEqual(transport.requests, [])

    def test_intent_uses_conservative_recipe_and_diverse_gear_searches(self):
        payload = {
            "summary": "A tight, articulate high-gain rhythm voice.",
            "pedal_query": "tube screamer overdrive",
            "amp_query": "Mesa Mark high gain amp",
            "intent": {
                "gain": "high",
                "tightness": "tight",
                "brightness": "balanced",
                "space": "dry",
                "boost": "light",
            },
            "gear_hints": ["Mesa Mark IIC+", "Ibanez Tube Screamer"],
        }
        plan = TonePlan.from_payload(payload)
        self.assertLessEqual(plan.controls["pedal_drive_db"], 8.0)
        self.assertEqual(plan.controls["amp_volume_db"], 0.0)
        self.assertTrue(plan.effects["eq"])

        transport = FakeOpenAITransport(payload, ranking_payload())
        tone3000 = FakeTone3000()
        service = ToneMakerService("sk-test", transport=transport, clock=lambda: 100)
        service.recommend("Metallica-style tight rhythm", tone3000)
        self.assertEqual(
            tone3000.searches,
            [
                ("tube screamer overdrive", "pedal"),
                ("Mesa Mark IIC+", "pedal"),
                ("Ibanez Tube Screamer", "pedal"),
                ("transparent overdrive boost", "pedal"),
                ("Mesa Mark high gain amp", "amp"),
                ("Mesa Mark IIC+", "amp"),
                ("Ibanez Tube Screamer", "amp"),
                ("high gain amp", "amp"),
                ("Mesa Mark high gain amp cab", "cabinet"),
                ("cabinet", "cabinet"),
                ("4x12", "cabinet"),
                ("guitar cab", "cabinet"),
            ],
        )


if __name__ == "__main__":
    unittest.main()
