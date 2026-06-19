#!/usr/bin/env python3

from __future__ import annotations

import argparse
import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True

SCRIPT = Path(__file__).with_name("benchmark-cranelift-runtime.py")
SPEC = importlib.util.spec_from_file_location("benchmark_cranelift_runtime", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
benchmark = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = benchmark
SPEC.loader.exec_module(benchmark)


class BenchmarkCraneliftRuntimeTest(unittest.TestCase):
    def test_parse_lane(self) -> None:
        lane = benchmark.parse_lane("llvm=/bin/uv,/bin/ruff,/bin/ty")
        self.assertEqual(lane.name, "llvm")
        self.assertEqual(lane.uv, Path("/bin/uv"))
        self.assertEqual(lane.ruff, Path("/bin/ruff"))
        self.assertEqual(lane.ty, Path("/bin/ty"))
        with self.assertRaises(argparse.ArgumentTypeError):
            benchmark.parse_lane("missing-binaries")

    def test_schedule_is_deterministic_and_balanced(self) -> None:
        lanes = ("llvm", "baseline", "candidate")
        schedule = benchmark.balanced_schedule(lanes, 12, 42)
        self.assertEqual(schedule, benchmark.balanced_schedule(lanes, 12, 42))
        self.assertEqual(len(schedule), 12)
        self.assertTrue(all(set(order) == set(lanes) for order in schedule))
        for lane in lanes:
            positions = [order.index(lane) for order in schedule[:6]]
            self.assertEqual(sorted(positions), [0, 0, 1, 1, 2, 2])

    def test_summary_uses_milliseconds(self) -> None:
        summary = benchmark.summarize([1_000_000, 2_000_000, 3_000_000])
        self.assertEqual(summary["median_ms"], 2.0)
        self.assertEqual(summary["mean_ms"], 2.0)
        self.assertEqual(summary["min_ms"], 1.0)
        self.assertEqual(summary["max_ms"], 3.0)

    def test_comparison_uses_paired_changes(self) -> None:
        comparison = benchmark.compare_samples(
            [100, 100, 100, 100],
            [90, 80, 110, 100],
        )
        self.assertEqual(comparison["median_change_percent"], -5.0)
        self.assertEqual(comparison["median_absolute_deviation_percent"], 10.0)
        self.assertEqual(comparison["wins"], 2)
        self.assertEqual(comparison["losses"], 1)
        self.assertEqual(comparison["ties"], 1)
        self.assertEqual(comparison["two_sided_sign_test_p"], 1.0)

    def test_exact_sign_test(self) -> None:
        self.assertEqual(benchmark.exact_sign_test(8, 0), 0.0078125)
        self.assertEqual(benchmark.exact_sign_test(0, 0), 1.0)

    def test_existing_unowned_scratch_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            scratch = Path(temporary) / "scratch"
            scratch.mkdir()
            with self.assertRaises(benchmark.BenchmarkError):
                benchmark.prepare_scratch(scratch)
            (scratch / benchmark.ROOT_MARKER).write_text(
                f"schema={benchmark.SCHEMA_VERSION}\n"
            )
            benchmark.prepare_scratch(scratch)


if __name__ == "__main__":
    unittest.main()
