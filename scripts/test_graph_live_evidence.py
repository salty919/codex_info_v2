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

    def test_rejected_remaining_increase_does_not_erase_token_idle(self):
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
        self.assertEqual([{"start_at": 0, "end_at": 120}], idle)
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

    def test_explicit_historical_period_is_selected_without_relabelling_it_current(self):
        periods = [
            {"id": "current", "current": True},
            {"id": "reported-regression", "current": False},
        ]
        self.assertEqual(
            periods[1],
            oracle._select_period(periods, "reported-regression"),
        )

    def test_default_live_capture_still_requires_exactly_one_current_period(self):
        periods = [
            {"id": "old", "current": False},
            {"id": "current", "current": True},
        ]
        self.assertEqual(periods[1], oracle._select_period(periods, None))
        with self.assertRaises(oracle.EvidenceError):
            oracle._select_period(
                [{"id": "one", "current": True}, {"id": "two", "current": True}],
                None,
            )

    def test_render_contract_fixes_native_viewbox_dash_ticks_and_markers(self):
        fixture = {
            "period": {
                "id": "render-contract",
                "start_at": 1_020,
                "end_at": 1_620,
                "reset_at": 1_620,
            },
            "history_page": {
                "api_version": "v3",
                "history_samples": [
                    {
                        "timestamp": 1_020,
                        "reset_at": 1_620,
                        "remaining_percent": 100,
                        "models": [
                            {"model": "SOL", "total_tokens": 10, "total_dollars": 10}
                        ],
                        "models_complete": True,
                        "model_source": "confirmed",
                    },
                    {
                        "timestamp": 1_620,
                        "reset_at": 1_620,
                        "remaining_percent": 90,
                        "models": [
                            {"model": "SOL", "total_tokens": 10, "total_dollars": 10}
                        ],
                        "models_complete": True,
                        "model_source": "confirmed",
                    },
                ],
                "history_gaps": [],
                "next_cursor": None,
                "resume_cursor": "fixture",
            },
        }

        contracts = oracle.build_expected_render_contracts(fixture)
        dollars = contracts["dollars"]

        self.assertTrue(
            dollars["models"][0]["dashed"].startswith(
                "M0.00 1.00 L0.45 1.00 M0.75 1.00"
            )
        )
        self.assertTrue(
            dollars["remaining"]["dashed"].startswith(
                "M0.00 1.00 L0.45 1.04 M0.75 1.07"
            )
        )
        self.assertEqual([1_020, 1_170, 1_320, 1_470, 1_620], dollars["time_ticks"])
        self.assertEqual(["$10.00", "$7.50", "$5.00", "$2.50", "$0.00"], dollars["axis_labels"])
        self.assertEqual(
            ["0.000000000000", "0.250000000000", "0.500000000000", "0.750000000000", "1.000000000000"],
            dollars["axis_grid_y"],
        )
        self.assertEqual(["SOL", "remaining"], [
            label["series"] for label in dollars["endpoint_labels"]
        ])
        self.assertEqual(["$10.00", "90%"], [
            label["text"] for label in dollars["endpoint_labels"]
        ])
        self.assertEqual(694, dollars["layout"]["plot_width"])
        self.assertEqual(94, dollars["layout"]["gutter_width"])
        self.assertEqual([99, 98, 97, 96, 95, 94, 93, 92, 91, 90], [
            marker["boundary"] for marker in dollars["remaining_markers"]
        ])


if __name__ == "__main__":
    unittest.main()
