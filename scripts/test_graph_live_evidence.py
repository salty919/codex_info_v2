#!/usr/bin/env python3

import copy
import json
import tempfile
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


def v3_fixture(rows, *, gaps=None, period_id="test"):
    samples = []
    reset_at = max(1_000, rows[-1]["timestamp"] + 60)
    for row in rows:
        models = row["models"] if "models" in row else [
            {
                "model": "SOL",
                "total_tokens": row["tokens"],
                "total_dollars": row.get("dollars", 0.0),
            }
        ]
        sample = {
            "timestamp": row["timestamp"],
            "reset_at": reset_at,
            "remaining_percent": row.get("remaining_percent"),
            "models": models,
            "models_complete": row.get("models_complete", True),
            "model_source": row.get("model_source", "confirmed"),
        }
        if "task_active_since_previous" in row:
            sample["task_active_since_previous"] = row["task_active_since_previous"]
        samples.append(sample)
    return {
        "period": {
            "id": period_id,
            "start_at": samples[0]["timestamp"],
            "end_at": samples[-1]["timestamp"],
            "reset_at": reset_at,
        },
        "history_page": {
            "api_version": "v3",
            "history_samples": samples,
            "history_gaps": gaps or [],
            "next_cursor": None,
            "resume_cursor": "fixture",
        },
    }


class GraphLiveEvidenceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.document = json.loads(FIXTURE_PATH.read_text(encoding="utf-8"))

    def test_remaining_smoothing_literal_cases_distinguish_measured_and_missing(self):
        for name, case in self.document["remaining_smoothing_v4"].items():
            fixture = v3_fixture(
                [
                    {
                        **row,
                        "models_complete": False,
                        "model_source": "legacy-unknown",
                    }
                    for row in case["samples"]
                ],
                gaps=[
                    {"start_at": start, "end_at": end}
                    for start, end in case.get("confirmed_gaps", [])
                ],
                period_id=name,
            )
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
        contracts = oracle.build_expected_render_contracts(fixture)["dollars"]
        self.assertEqual(
            [point["effective"] for point in expected["remaining_points"]],
            [float(point["value"]) for point in contracts["remaining_points"]],
        )
        self.assertEqual(
            [point["origin"] for point in expected["remaining_points"]],
            [point["origin"] for point in contracts["remaining_points"]],
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
        self.assertEqual([], idle)
        self.assertIn([60, 120], pairs(segments, "dashed"))

    def test_idle_uses_exact_tokens_while_true_lifecycle_vetoes(self):
        for activity, expected in (
            (None, [{"start_at": 0, "end_at": 1_800}]),
            (False, [{"start_at": 0, "end_at": 1_800}]),
            (True, []),
        ):
            rows = [
                {
                    "timestamp": minute * 60,
                    "remaining_percent": 90.0,
                    "tokens": 100,
                    "dollars": float(minute),
                    "task_active_since_previous": activity,
                }
                for minute in range(31)
            ]
            self.assertEqual(expected, oracle.build_expected(v3_fixture(rows))[1], activity)

        incomplete = v3_fixture(
            [
                {
                    "timestamp": 0,
                    "remaining_percent": 90.0,
                    "models": [{"model": "SOL", "total_tokens": 100, "total_dollars": 1.0}],
                    "models_complete": False,
                    "model_source": "legacy-unknown",
                    "task_active_since_previous": None,
                },
                {
                    "timestamp": 60,
                    "remaining_percent": 90.0,
                    "models": [{"model": "SOL", "total_tokens": 100, "total_dollars": 9.0}],
                    "models_complete": False,
                    "model_source": "legacy-unknown",
                    "task_active_since_previous": False,
                },
                {
                    "timestamp": 120,
                    "remaining_percent": 90.0,
                    "models": [
                        {"model": "SOL", "total_tokens": 100, "total_dollars": 9.0},
                        {"model": "TERRA", "total_tokens": 0, "total_dollars": 0.0},
                    ],
                    "task_active_since_previous": False,
                },
            ]
        )
        self.assertEqual([], oracle.build_expected(incomplete)[1])

    def test_idle_requires_thirty_minutes_and_exact_sparse_anchors_can_prove_it(self):
        self.assertEqual(
            [{"start_at": 0, "end_at": 1_800}],
            oracle.build_expected(
                v3_fixture(
                    [
                        {
                            "timestamp": minute * 60,
                            "remaining_percent": 90.0,
                            "tokens": 100,
                            "task_active_since_previous": False,
                        }
                        for minute in range(31)
                    ]
                )
            )[1],
        )
        self.assertEqual(
            [],
            oracle.build_expected(
                v3_fixture(
                    [
                        {"timestamp": 0, "remaining_percent": 90.0, "tokens": 100},
                        {
                            "timestamp": 60,
                            "remaining_percent": 90.0,
                            "tokens": 100,
                            "task_active_since_previous": False,
                        },
                    ]
                )
            )[1],
        )
        self.assertEqual(
            [{"start_at": 0, "end_at": 1_800}],
            oracle.build_expected(
                v3_fixture(
                    [
                        {"timestamp": 0, "remaining_percent": 90.0, "tokens": 100},
                        {
                            "timestamp": 1_800,
                            "remaining_percent": 90.0,
                            "tokens": 100,
                            "task_active_since_previous": False,
                        },
                    ]
                )
            )[1],
        )

    def test_idle_bridges_missing_cadence_only_between_two_proven_flat_runs(self):
        def fixture(token_at_right):
            left = [
                {"timestamp": minute * 60, "remaining_percent": 90.0, "tokens": 100}
                for minute in range(31)
            ]
            right = [
                {
                    "timestamp": 1_920 + minute * 60,
                    "remaining_percent": 90.0,
                    "tokens": token_at_right,
                }
                for minute in range(31)
            ]
            return v3_fixture(left + right)

        self.assertEqual(
            [{"start_at": 0, "end_at": 3_720}],
            oracle.build_expected(fixture(100))[1],
        )
        self.assertEqual(
            [
                {"start_at": 0, "end_at": 1_800},
                {"start_at": 1_920, "end_at": 3_720},
            ],
            oracle.build_expected(fixture(200))[1],
        )

    def test_reconstructed_rows_cannot_publish_model_numerics_or_confirm_idle(self):
        invalid = v3_fixture(
            [
                {
                    "timestamp": 0,
                    "remaining_percent": 100.0,
                    "models": [
                        {"model": "SOL", "total_tokens": 0, "total_dollars": 0.0},
                        {"model": "LUNA", "total_tokens": 0, "total_dollars": 0.0},
                        {"model": "TERRA", "total_tokens": 0, "total_dollars": 0.0},
                    ],
                    "models_complete": False,
                    "model_source": "reconstructed-from-session",
                },
                {
                    "timestamp": 60,
                    "remaining_percent": 99.0,
                    "models": [
                        {"model": "SOL", "total_tokens": 100, "total_dollars": 1.0},
                    ],
                    "models_complete": True,
                    "model_source": "confirmed",
                },
            ]
        )
        with self.assertRaises(oracle.EvidenceError):
            oracle.build_expected(invalid)

        fixture = v3_fixture(
            [
                {
                    "timestamp": 0,
                    "remaining_percent": 100.0,
                    "models": None,
                    "models_complete": False,
                    "model_source": "reconstructed-from-session",
                },
                {
                    "timestamp": 60,
                    "remaining_percent": 99.0,
                    "models": [
                        {"model": "SOL", "total_tokens": 100, "total_dollars": 1.0},
                    ],
                    "models_complete": True,
                    "model_source": "confirmed",
                },
                {
                    "timestamp": 120,
                    "remaining_percent": 98.0,
                    "models": None,
                    "models_complete": False,
                    "model_source": "reconstructed-from-session",
                },
                {
                    "timestamp": 180,
                    "remaining_percent": 97.0,
                    "models": [
                        {"model": "SOL", "total_tokens": 300, "total_dollars": 3.0},
                    ],
                    "models_complete": True,
                    "model_source": "confirmed",
                },
            ]
        )

        segments, idle = oracle.build_expected(fixture)
        self.assertEqual(
            [],
            pairs(segments, "rising", "tokens", "SOL"),
        )
        self.assertEqual(
            [[60, 120], [120, 180]],
            pairs(segments, "dashed", "tokens", "SOL"),
        )
        self.assertEqual([], idle)

    def test_remaining_drop_uses_one_token_or_elapsed_weight_for_the_whole_span(self):
        token_weighted = v3_fixture(
            [
                {"timestamp": 0, "remaining_percent": 100.0, "tokens": 0},
                {"timestamp": 60, "remaining_percent": 100.0, "tokens": 1},
                {"timestamp": 120, "remaining_percent": None, "tokens": 1},
                {"timestamp": 180, "remaining_percent": 94.0, "tokens": 5},
            ],
            period_id="token-weighted",
        )
        segments, _ = oracle.build_expected(token_weighted)
        points = oracle.build_expected_render_contracts(token_weighted)["tokens"]["remaining_points"]
        self.assertEqual([100.0, 98.8, 98.8, 94.0], [float(point["value"]) for point in points])
        self.assertEqual(["raw", "activity_smoothed", "interpolated", "raw"], [point["origin"] for point in points])
        self.assertEqual([[0, 60]], pairs(segments, "solid"))
        self.assertEqual([[60, 120], [120, 180]], pairs(segments, "dashed"))

        fallback = v3_fixture(
            [
                {"timestamp": 0, "remaining_percent": 100.0, "tokens": 0},
                {"timestamp": 60, "remaining_percent": 100.0, "tokens": 1},
                {
                    "timestamp": 120,
                    "remaining_percent": 100.0,
                    "tokens": 2,
                    "models": [],
                    "models_complete": False,
                    "model_source": "legacy-unknown",
                },
                {"timestamp": 180, "remaining_percent": 94.0, "tokens": 5},
            ],
            period_id="elapsed-fallback",
        )
        fallback_segments, _ = oracle.build_expected(fallback)
        fallback_points = oracle.build_expected_render_contracts(fallback)["tokens"]["remaining_points"]
        self.assertEqual([100.0, 98.8, 96.4, 94.0], [float(point["value"]) for point in fallback_points])
        self.assertEqual(["raw", "activity_smoothed", "interpolated", "raw"], [point["origin"] for point in fallback_points])
        self.assertEqual([[0, 60]], pairs(fallback_segments, "solid"))
        self.assertEqual([[60, 120], [120, 180]], pairs(fallback_segments, "dashed"))

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

    def test_account_id_is_canonical_and_period_selector_rejects_other_or_duplicate_accounts(self):
        self.assertEqual("account-2", oracle._validate_account_id("account-2"))
        for invalid in ("account-0", "account-01", "account-", "account-2x", "account_2"):
            with self.assertRaises(oracle.EvidenceError):
                oracle._validate_account_id(invalid)

        with self.assertRaises(oracle.EvidenceError):
            oracle._select_period(
                [{"id": "only", "current": True, "account_id": "account-1"}],
                None,
                "account-2",
            )
        with self.assertRaises(oracle.EvidenceError):
            oracle._select_period(
                [
                    {"id": "same", "current": False, "account_id": "account-2"},
                    {"id": "same", "current": False, "account_id": "account-2"},
                ],
                "same",
                "account-2",
            )

    def test_account_id_is_sent_to_both_capture_endpoints_and_fixed_in_provenance(self):
        pair = "v1:" + "0" * 64
        periods = {
            "api_version": "v3",
            "history_periods": [
                {
                    "id": "account-period",
                    "start_at": 0,
                    "end_at": 60,
                    "reset_at": 60,
                    "current": True,
                    "account_id": "account-2",
                }
            ],
        }
        history = {
            "api_version": "v3",
            "history_samples": [
                {
                    "timestamp": 0,
                    "reset_at": 60,
                    "remaining_percent": 100.0,
                    "models": [{"model": "SOL", "total_tokens": 0, "total_dollars": 0.0}],
                    "models_complete": True,
                    "model_source": "confirmed",
                },
                {
                    "timestamp": 60,
                    "reset_at": 60,
                    "remaining_percent": 99.0,
                    "models": [{"model": "SOL", "total_tokens": 1, "total_dollars": 1.0}],
                    "models_complete": True,
                    "model_source": "confirmed",
                },
            ],
            "history_gaps": [],
            "next_cursor": None,
            "resume_cursor": "account-2",
        }
        responses = [
            (json.dumps(periods).encode("utf-8"), pair),
            (json.dumps(history).encode("utf-8"), pair),
        ]
        with tempfile.TemporaryDirectory() as directory:
            output_directory = Path(directory) / "evidence"
            with mock.patch.object(oracle, "_fetch", side_effect=responses) as fetch:
                output = oracle.capture(
                    "http://127.0.0.1:8787",
                    output_directory,
                    "a" * 40,
                    account_id="account-2",
                )

            self.assertEqual(
                [
                    mock.call("http://127.0.0.1:8787/v3/history/periods?account=account-2"),
                    mock.call(
                        "http://127.0.0.1:8787/v3/history?account=account-2&period=account-period"
                    ),
                ],
                fetch.call_args_list,
            )
            artifact = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual("account-2", artifact["account_id"])
            self.assertEqual("account-2", artifact["fixture"]["account_id"])

    def _assert_verify_rejects_windows_mutation(self, mutate, *, artifact_account_id=None):
        fixture = copy.deepcopy(self.document["parity_v3"])
        expected_segments, expected_idle = oracle.build_expected(fixture)
        actual_segments = [
            {key: segment[key] for key in ("metric", "series", "start_at", "end_at", "style")}
            for segment in expected_segments
        ]
        expected_render = oracle.build_expected_render_contracts(fixture)
        artifact = {
            "schema_version": "graph-evidence-v1",
            "source_sha": "a" * 40,
            "account_id": artifact_account_id,
            "published_pair": fixture["published_pair"],
            "period": fixture["period"],
            "inputs": [],
            "input_sha256": "b" * 64,
            "fixture": fixture,
            "expected_segments": expected_segments,
            "expected_render_contracts": expected_render,
            "actual_projection_segments": [],
            "expected_idle_intervals": expected_idle,
            "actual_idle_intervals": {"linux": [], "windows": []},
            "unmatched_expected": {"linux": [], "windows": []},
            "unmatched_actual": {"linux": [], "windows": []},
            "cross_platform_mismatches": [],
        }
        actual = {
            "schema_version": "graph-actual-v1",
            "source_sha": "a" * 40,
            "input_sha256": "b" * 64,
            "published_pair": fixture["published_pair"],
            "segments": actual_segments,
            "idle_intervals": expected_idle,
            "render_contracts": copy.deepcopy(expected_render),
        }
        linux = copy.deepcopy(actual)
        linux["platform"] = "linux"
        windows = copy.deepcopy(actual)
        windows["platform"] = "windows"
        mutate(windows)
        with tempfile.TemporaryDirectory() as directory:
            evidence_path = Path(directory) / "evidence.json"
            linux_path = Path(directory) / "linux.json"
            windows_path = Path(directory) / "windows.json"
            evidence_path.write_text(json.dumps(artifact), encoding="utf-8")
            linux_path.write_text(json.dumps(linux), encoding="utf-8")
            windows_path.write_text(json.dumps(windows), encoding="utf-8")
            with self.assertRaises(oracle.EvidenceError):
                oracle.verify(evidence_path, linux_path, windows_path)

    def test_verify_rejects_raw_endpoint_value_mismatch_hidden_by_display_rounding(self):
        self._assert_verify_rejects_windows_mutation(
            lambda document: next(
                value
                for value in document["render_contracts"]["dollars"]["endpoint_values"]
                if value["series"] == "SOL"
            ).update(value=14.004)
        )

    def test_verify_rejects_latest_timestamp_mismatch(self):
        self._assert_verify_rejects_windows_mutation(
            lambda document: document["render_contracts"]["dollars"].update(
                latest_timestamp=document["render_contracts"]["dollars"]["latest_timestamp"] + 60
            )
        )

    def test_verify_rejects_published_pair_mismatch(self):
        self._assert_verify_rejects_windows_mutation(
            lambda document: document.update(published_pair="v1:" + "f" * 64)
        )

    def test_verify_rejects_display_label_mismatch(self):
        self._assert_verify_rejects_windows_mutation(
            lambda document: document["render_contracts"]["dollars"]["endpoint_labels"][0].update(
                series="SOLX"
            )
        )

    def test_verify_rejects_missing_endpoint_values_instead_of_passing_old_schema(self):
        self._assert_verify_rejects_windows_mutation(
            lambda document: document["render_contracts"]["dollars"].pop("endpoint_values")
        )

    def test_verify_rejects_missing_account_provenance_instead_of_mixing_accounts(self):
        self._assert_verify_rejects_windows_mutation(
            lambda document: None,
            artifact_account_id="account-2",
        )

    def test_parity_fixture_binds_raw_endpoint_values_labels_and_latest_timestamp(self):
        fixture = self.document["parity_v3"]
        expected = fixture["expected"]
        contracts = oracle.build_expected_render_contracts(fixture)
        self.assertEqual(
            {"LUNA": 1.0, "SOL": 14.0, "TERRA": 2.0, "remaining": 92.0},
            {item["series"]: item["value"] for item in contracts["dollars"]["endpoint_values"]},
        )
        self.assertEqual(
            {"LUNA": 1.0, "SOL": 14.0, "TERRA": 2.0, "remaining": 92.0},
            {item["series"]: item["value"] for item in contracts["tokens"]["endpoint_values"]},
        )
        for metric in ("dollars", "tokens"):
            self.assertEqual(fixture["period"]["end_at"], contracts[metric]["latest_timestamp"])
        self.assertEqual(
            expected["latest_labels"],
            {
                series: text
                for series, text in (
                    (item["series"], item["text"])
                    for item in contracts["dollars"]["endpoint_labels"]
                )
                if series in expected["latest_labels"]
            },
        )
        self.assertEqual(
            {"SOL": "14", "remaining": "92%"},
            {
                series: text
                for series, text in (
                    (item["series"], item["text"])
                    for item in contracts["tokens"]["endpoint_labels"]
                )
                if series in {"SOL", "remaining"}
            },
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
