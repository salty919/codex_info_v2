#!/usr/bin/env python3

import json
import unittest
from email.message import Message
from pathlib import Path
from unittest import mock

from scripts import graph_live_evidence as oracle


ROOT = Path(__file__).resolve().parents[1]
FIXTURE_PATH = ROOT / "tests" / "fixtures" / "graph_evidence_oracle.json"


def pairs(segments, style, metric="remaining", series="remaining"):
    return [
        [segment["start_at"], segment["end_at"]]
        for segment in segments
        if segment["metric"] == metric
        and segment["series"] == series
        and segment["style"] == style
    ]


class GraphLiveEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.document = json.loads(FIXTURE_PATH.read_text(encoding="utf-8"))

    def test_remaining_smoothing_literal_cases_distinguish_measured_and_missing(self):
        for name, case in self.document["remaining_smoothing_v4"].items():
            samples = [
                {
                    "timestamp": row["timestamp"],
                    "reset_at": 1_000,
                    "remaining_percent": row["remaining_percent"],
                    "models": [
                        {
                            "model": "SOL",
                            "total_tokens": row["tokens"],
                            "total_dollars": row["dollars"],
                        }
                    ],
                    "models_complete": False,
                    "model_source": "legacy-unknown",
                }
                for row in case["samples"]
            ]
            fixture = {
                "period": {
                    "id": name,
                    "start_at": samples[0]["timestamp"],
                    "end_at": samples[-1]["timestamp"],
                    "reset_at": 1_000,
                },
                "history_page": {
                    "api_version": "v3",
                    "history_samples": samples,
                    "history_gaps": [],
                    "next_cursor": None,
                    "resume_cursor": "fixture",
                },
            }
            segments, idle = oracle.build_expected(fixture)
            self.assertEqual(case["remaining_solid"], pairs(segments, "solid"), name)
            self.assertEqual(case["remaining_dashed"], pairs(segments, "dashed"), name)
            self.assertEqual(
                [
                    {"start_at": interval[0], "end_at": interval[1]}
                    for interval in case["idle_intervals"]
                ],
                idle,
                name,
            )

    def test_parity_fixture_matches_existing_dollar_remaining_and_idle_oracles(self):
        fixture = self.document["parity_v3"]
        expected = fixture["expected"]
        segments, idle = oracle.build_expected(fixture)
        for model, model_expected in expected["model_segments"].items():
            measured = sorted(
                pairs(segments, "flat", "dollars", model)
                + pairs(segments, "rising", "dollars", model)
            )
            self.assertEqual(
                sorted(model_expected["solid"]), measured, model
            )
            self.assertEqual(
                model_expected["dashed"], pairs(segments, "dashed", "dollars", model), model
            )
        self.assertEqual(
            expected["remaining_segments"]["solid"], pairs(segments, "solid")
        )
        self.assertEqual(
            expected["remaining_segments"]["dashed"], pairs(segments, "dashed")
        )
        self.assertEqual(
            [
                {"start_at": interval[0], "end_at": interval[1]}
                for interval in expected["idle_intervals"]
            ],
            idle,
        )

    def test_continuity_fixture_matches_flat_rising_dashed_and_idle_roles(self):
        fixture = self.document["continuous_idle_v4"]
        expected = fixture["expected"]
        origin = fixture["period"]["start_at"]
        segments, idle = oracle.build_expected(fixture)

        def absolute(intervals):
            return [[origin + start, origin + end] for start, end in intervals]

        for metric in ("tokens", "dollars"):
            for model, model_expected in expected["model_segments"].items():
                for style in ("flat", "rising", "dashed"):
                    self.assertEqual(
                        absolute(model_expected[style]),
                        pairs(segments, style, metric, model),
                        f"{metric}:{model}:{style}",
                    )
        for style in ("solid", "dashed"):
            self.assertEqual(
                absolute(expected["remaining_segments"][style]),
                pairs(segments, style),
                f"remaining:{style}",
            )
        self.assertEqual(
            [
                {"start_at": origin + start, "end_at": origin + end}
                for start, end in expected["idle_intervals"]
            ],
            idle,
        )

    def test_cause_ids_are_deduplicated_in_canonical_order(self):
        self.assertEqual(
            ["confirmed_gap", "model_missing", "terminal_unobserved"],
            oracle._causes(
                ["terminal_unobserved", "model_missing", "confirmed_gap", "model_missing"]
            ),
        )

    def test_persistent_rejected_remaining_increase_is_not_idle(self):
        rows = [(0, 90.0, 100), (60, 95.0, 100), (120, 95.0, 100), (180, 89.0, 101)]
        fixture = {
            "period": {"id": "remaining-anomaly", "start_at": 0, "end_at": 180, "reset_at": 180},
            "history_page": {
                "api_version": "v3",
                "history_samples": [
                    {
                        "timestamp": timestamp,
                        "reset_at": 180,
                        "remaining_percent": remaining,
                        "models": [
                            {
                                "model": "SOL",
                                "total_tokens": tokens,
                                "total_dollars": float(tokens),
                            }
                        ],
                        "models_complete": True,
                        "model_source": "confirmed",
                    }
                    for timestamp, remaining, tokens in rows
                ],
                "history_gaps": [],
                "next_cursor": None,
                "resume_cursor": "fixture",
            },
        }
        segments, idle = oracle.build_expected(fixture)
        self.assertEqual([], idle)
        self.assertIn([60, 120], pairs(segments, "dashed"))

    def test_json_loader_rejects_duplicate_object_keys(self):
        with self.assertRaises(oracle.EvidenceError):
            oracle._json_loads(b'{"same":1,"same":2}')

    def test_fetch_uses_the_strict_loopback_request_header_contract(self):
        headers = Message()
        headers.add_header(oracle.PAIR_HEADER, "v1:" + "0" * 64)
        response = mock.Mock(status=200, headers=headers)
        response.read.return_value = b"{}"
        connection = mock.Mock()
        connection.getresponse.return_value = response
        with mock.patch.object(oracle.http.client, "HTTPConnection", return_value=connection):
            body, pair = oracle._fetch("http://127.0.0.1:8787/v3/history?period=fixture")
        self.assertEqual(b"{}", body)
        self.assertEqual("v1:" + "0" * 64, pair)
        connection.putrequest.assert_called_once_with(
            "GET", "/v3/history?period=fixture", skip_accept_encoding=True
        )
        connection.putheader.assert_called_once_with("Accept", "application/json")
        connection.endheaders.assert_called_once_with()
        connection.close.assert_called_once_with()


if __name__ == "__main__":
    unittest.main()
