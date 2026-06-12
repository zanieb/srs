#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from collections import Counter
from pathlib import Path

sys.dont_write_bytecode = True

SCRIPT = Path(__file__).with_name("benchmark-artifact-cache.py")
REAL_STATS_FIXTURE = SCRIPT.parent / "fixtures" / "artifact-cache-stats-v1.json"
SPEC = importlib.util.spec_from_file_location("benchmark_artifact_cache", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
benchmark = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(benchmark)
shutil.rmtree(SCRIPT.parent / "__pycache__", ignore_errors=True)


class BenchmarkArtifactCacheTest(unittest.TestCase):
    def test_seven_trial_schedule_is_deterministic_and_balanced(self) -> None:
        schedule = benchmark.balanced_schedule(7, 19754)
        self.assertEqual(schedule, benchmark.balanced_schedule(7, 19754))
        self.assertEqual(len(schedule), 7)
        self.assertTrue(all(set(block) == set(benchmark.STATES) for block in schedule))
        for state in benchmark.STATES:
            positions = Counter(block.index(state) for block in schedule[:6])
            self.assertEqual(positions, Counter({0: 2, 1: 2, 2: 2}))

    def test_warm_cache_is_reset_without_mutating_seed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            seed = root / "seed"
            active = root / "active"
            seed.mkdir()
            (seed / "entry").write_bytes(b"portable entry")
            original = benchmark.tree_digest(seed)
            benchmark.reset_active_cache(seed, active)
            (active / "entry").write_bytes(b"consumer variant")
            (active / "new-entry").write_text("new")
            benchmark.reset_active_cache(seed, active)
            self.assertEqual((active / "entry").read_bytes(), b"portable entry")
            self.assertFalse((active / "new-entry").exists())
            self.assertEqual(benchmark.tree_digest(seed), original)

    def test_destructive_roots_are_rejected_before_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            protected = root / "protected"
            protected.mkdir()
            marker = protected / "marker"
            marker.write_text("keep")

            with self.assertRaises(benchmark.BenchmarkError):
                benchmark.ensure_safe_root(Path(Path.cwd().anchor), [protected])
            with self.assertRaises(benchmark.BenchmarkError):
                benchmark.ensure_safe_root(protected / "child", [protected])
            with self.assertRaises(benchmark.BenchmarkError):
                benchmark.ensure_safe_root(root, [protected])
            self.assertEqual(marker.read_text(), "keep")

            nonempty = root / "nonempty"
            nonempty.mkdir()
            nonempty_marker = nonempty / "marker"
            nonempty_marker.write_text("keep")
            with self.assertRaises(benchmark.BenchmarkError):
                benchmark.prepare_root(nonempty, overwrite=False)
            self.assertEqual(nonempty_marker.read_text(), "keep")
            with self.assertRaises(benchmark.BenchmarkError):
                benchmark.prepare_root(nonempty, overwrite=True)
            self.assertEqual(nonempty_marker.read_text(), "keep")

            owned = root / "owned"
            owned.mkdir()
            (owned / benchmark.ROOT_MARKER).write_text("schema=1\n")
            (owned / "old-result").write_text("remove")
            benchmark.prepare_root(owned, overwrite=True)
            self.assertFalse((owned / "old-result").exists())
            self.assertTrue((owned / benchmark.ROOT_MARKER).is_file())

            output = root / "output"
            target = root / "target"
            cache = root / "cache"
            benchmark.validate_independent_roots((output, target, cache))
            with self.assertRaises(benchmark.BenchmarkError):
                benchmark.validate_independent_roots((output, output, cache))
            with self.assertRaises(benchmark.BenchmarkError):
                benchmark.validate_independent_roots((output, output / "child", cache))

    def test_extracts_exactly_one_statistics_record(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            stderr = Path(temporary) / "stderr.log"
            stderr.write_text(
                "Compiling sample\n"
                + benchmark.STATS_PREFIX
                + json.dumps({"version": 1, "configured": True})
                + "\nFinished\n"
            )
            self.assertEqual(
                benchmark.extract_stats(stderr),
                {"version": 1, "configured": True},
            )
            benchmark.validate_stats_state(
                {"configured": True}, workload="sample", state="warm"
            )
            benchmark.validate_stats_state(
                {"configured": False}, workload="sample", state="disabled"
            )
            with self.assertRaises(benchmark.BenchmarkError):
                benchmark.validate_stats_state(
                    {"configured": False}, workload="sample", state="cold"
                )

    def test_coverage_distinguishes_eligibility_and_effectiveness(self) -> None:
        stats = {
            "units": {"eligible": 25, "ineligible": 45},
            "lookup": {"hits": 25},
            "rustc": {"executions": 45},
        }
        metrics = benchmark.coverage_metrics(stats)
        self.assertAlmostEqual(metrics["eligibility"], 25 / 70)
        self.assertEqual(metrics["effectiveness"], 1.0)
        self.assertAlmostEqual(metrics["total_coverage"], 25 / 70)

    def test_median_absolute_deviation(self) -> None:
        self.assertEqual(benchmark.median_absolute_deviation([1, 2, 2, 4, 9]), 1)

    def test_manifest_validation_rejects_truncated_and_duplicate_trials(self) -> None:
        metadata = {
            "workloads": ["sample"],
            "trials": 1,
            "schedule": [["disabled", "cold", "warm"]],
        }
        population = {
            "workload": "sample",
            "trial": 0,
            "state": "cold",
            "order": 0,
            "population": True,
            "relative_directory": "population/sample",
        }
        measured = [
            {
                "workload": "sample",
                "trial": 1,
                "state": state,
                "order": order,
                "population": False,
                "relative_directory": f"runs/{order}-{state}",
            }
            for order, state in enumerate(metadata["schedule"][0], 1)
        ]
        benchmark.validate_run_manifest(metadata, [population, *measured])
        with self.assertRaises(benchmark.BenchmarkError):
            benchmark.validate_run_manifest(metadata, [population, *measured[:-1]])
        with self.assertRaises(benchmark.BenchmarkError):
            benchmark.validate_run_manifest(
                metadata, [population, *measured, measured[-1]]
            )

    def test_revision_prefix_requires_seven_characters(self) -> None:
        revision = "f74311c15abcdef0123456789abcdef012345678"
        self.assertTrue(benchmark.revision_matches(revision, "f74311c"))
        self.assertFalse(benchmark.revision_matches(revision, "f74311"))
        self.assertFalse(benchmark.revision_matches(revision, "deadbee"))

    def test_repository_status_detects_new_ignored_state(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repository = Path(temporary)
            subprocess.run(["git", "init", "-q", str(repository)], check=True)
            (repository / ".gitignore").write_text("ignored-state\n")
            baseline = benchmark.repository_status(repository)
            (repository / "ignored-state").write_text("mutation")
            self.assertNotEqual(benchmark.repository_status(repository), baseline)

    def test_toolchain_provenance_rejects_executable_mismatch(self) -> None:
        provenance = {"executables": {"cargo": "expected"}}
        cargo = {"sha256": "actual"}
        with self.assertRaises(benchmark.BenchmarkError):
            benchmark.validate_toolchain_executables(provenance, cargo, None)

    def test_backend_flags_are_target_encoded_and_host_wrapper_is_removed(self) -> None:
        environment = benchmark.benchmark_environment(
            {
                "RUSTC_WRAPPER": "sccache",
                "RUSTC_WORKSPACE_WRAPPER": "workspace-wrapper",
                "RUSTC": "/poison/rustc",
                "CARGO_BUILD_RUSTC": "/poison/cargo-rustc",
                "CARGO_BUILD_RUSTC_WRAPPER": "/poison/cargo-wrapper",
                "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER": "/poison/workspace-wrapper",
                "RUSTFLAGS": "poison",
                "SRS_CARGO_ARTIFACT_CACHE_SNAPSHOT_MANIFEST": "/poison/collect.json",
            },
            {
                "SRS_CARGO_ARTIFACT_CACHE_SNAPSHOT_RESTORE_MANIFEST": "{workspace}/poison.json",
                "RUSTC": "/workload/poison-rustc",
                "CARGO_BUILD_RUSTC_WRAPPER": "/workload/poison-wrapper",
            },
            state="cold",
            active_cache=Path("/cache"),
            target=Path("/target"),
            workspace=Path("/workspace"),
            backend="llvm",
            linker="/usr/bin/clang",
            jobs=8,
            materialization="hardlink",
            run_directory=Path("/results/run"),
            toolchain_bin=Path("/toolchain/bin"),
        )
        self.assertEqual(
            environment["CARGO_ENCODED_RUSTFLAGS"],
            "-Zcodegen-backend=llvm\x1f-Clinker=/usr/bin/clang",
        )
        self.assertNotIn("RUSTFLAGS", environment)
        self.assertNotIn("RUSTC_WRAPPER", environment)
        self.assertNotIn("RUSTC_WORKSPACE_WRAPPER", environment)
        self.assertNotIn("RUSTC", environment)
        self.assertNotIn("CARGO_BUILD_RUSTC", environment)
        self.assertNotIn("CARGO_BUILD_RUSTC_WRAPPER", environment)
        self.assertNotIn("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER", environment)
        self.assertNotIn("SRS_CARGO_ARTIFACT_CACHE_SNAPSHOT_MANIFEST", environment)
        self.assertNotIn(
            "SRS_CARGO_ARTIFACT_CACHE_SNAPSHOT_RESTORE_MANIFEST", environment
        )
        self.assertEqual(
            environment["CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION"], "hardlink"
        )
        self.assertEqual(
            environment["SRS_CARGO_ARTIFACT_CACHE_MATERIALIZATION"], "hardlink"
        )
        self.assertNotIn("CARGO_BUILD_ARTIFACT_CACHE_MAX_SIZE", environment)
        self.assertNotIn("SRS_CARGO_ARTIFACT_CACHE_MAX_SIZE", environment)
        self.assertEqual(
            environment["PATH"].split(os.pathsep)[0], "/toolchain/bin"
        )

    def test_workload_file_contains_exact_commands(self) -> None:
        workloads = benchmark.load_workloads(
            SCRIPT.with_name("artifact-cache-benchmark-workloads.json")
        )
        self.assertEqual(
            workloads["build"]["command"],
            [
                "build",
                "--profile",
                "no-debug",
                "--bin",
                "uv",
                "--bin",
                "uvx",
                "--locked",
            ],
        )
        benchmark.validate_backend_workloads("llvm", ["build", "clippy", "test"])
        benchmark.validate_backend_workloads("cranelift", ["build", "clippy"])
        with self.assertRaises(benchmark.BenchmarkError):
            benchmark.validate_backend_workloads("cranelift", ["test"])
        self.assertEqual(
            workloads["clippy"]["command"],
            [
                "clippy",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--locked",
            ],
        )
        self.assertEqual(
            workloads["test"]["command"],
            [
                "nextest",
                "run",
                "--cargo-profile",
                "fast-build",
                "--features",
                "test-python-patch,native-auth,secret-service",
                "--workspace",
                "--profile",
                "ci-linux",
            ],
        )

    def test_cli_retains_raw_runs_and_summarizes_coverage(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subject = root / "subject"
            subject.mkdir()
            subprocess.run(["git", "init", "-q", str(subject)], check=True)
            subprocess.run(
                ["git", "-C", str(subject), "config", "user.email", "test@example.com"],
                check=True,
            )
            subprocess.run(
                ["git", "-C", str(subject), "config", "user.name", "Test"],
                check=True,
            )
            (subject / "input").write_text("pinned\n")
            subprocess.run(["git", "-C", str(subject), "add", "input"], check=True)
            subprocess.run(
                ["git", "-C", str(subject), "commit", "-q", "-m", "fixture"],
                check=True,
            )
            subject_revision = subprocess.run(
                ["git", "-C", str(subject), "rev-parse", "HEAD"],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()
            srs_revision = subprocess.run(
                ["git", "-C", str(SCRIPT.parent.parent), "rev-parse", "HEAD"],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()
            provenance = root / "toolchain-provenance.json"
            provenance.write_text(
                json.dumps(
                    {
                        "srs_revision": srs_revision,
                        "artifact_sha256": "a" * 64,
                        "source": "unit-test fixture",
                        "executables": {},
                    }
                )
            )
            cargo = root / "fake-cargo.py"
            cargo.write_text("""#!/usr/bin/env python3
import json
import os
import pathlib
import sys
if sys.argv[1:] == [\"-Vv\"]:
    print(\"cargo 1.0.0 (fixture)\")
    raise SystemExit(0)
target = pathlib.Path(os.environ[\"CARGO_TARGET_DIR\"])
target.mkdir(parents=True, exist_ok=True)
(target / \"output\").write_text(\"built\")
disabled = os.environ.get(\"SRS_CARGO_ARTIFACT_CACHE\") == \"0\"
cache_value = os.environ.get(\"CARGO_BUILD_ARTIFACT_CACHE_DIR\")
warm = bool(cache_value and (pathlib.Path(cache_value) / \"entry\").exists())
if cache_value:
    cache = pathlib.Path(cache_value)
    cache.mkdir(parents=True, exist_ok=True)
    (cache / \"entry\").write_text(\"entry\")
eligible = 0 if disabled else 2
ineligible = 0 if disabled else 1
hits = 2 if warm else 0
stats = {
    \"version\": 1,
    \"configured\": not disabled,
    \"units\": {
        \"cargo_fresh\": 0,
        \"eligible\": eligible,
        \"ineligible\": ineligible,
        \"ineligible_by_reason\": {\"proc_macro\": ineligible},
    },
    \"preflight\": {\"elapsed_us\": 200 if warm else 0},
    \"lookup\": {
        \"hits\": hits,
        \"misses\": eligible - hits,
        \"elapsed_us\": 300 if warm else 100,
    },
    \"restore\": {\"files\": hits * 2, \"logical_bytes\": hits * 1024},
    \"materialization\": {
        \"hardlinked_files\": hits * 2,
        \"hardlinked_logical_bytes\": hits * 1024,
        \"copied_files\": 0,
        \"copied_logical_bytes\": 0,
        \"cross_device_copied_files\": 0,
        \"cross_device_copied_logical_bytes\": 0,
        \"elapsed_us\": 40 if warm else 0,
    },
    \"hashing\": {
        \"compiler_identity\": {
            \"computations\": 1,
            \"reuses\": 1,
            \"files\": 5,
            \"bytes\": 4096,
            \"wall_us\": 50,
            \"computation_wall_us\": 45,
            \"computation_cpu_us\": 40,
        },
        \"action_inputs\": {
            \"calls\": eligible,
            \"files\": eligible * 3,
            \"bytes\": eligible * 2048,
            \"wall_us\": 75,
        },
    },
    \"publication\": {
        \"attempts\": eligible - hits,
        \"stored\": eligible - hits,
        \"files\": (eligible - hits) * 2,
        \"logical_bytes\": (eligible - hits) * 1024,
        \"elapsed_us\": 80 if not warm else 0,
    },
    \"rustc\": {\"executions\": 1 if warm else 3, \"elapsed_us\": 1000},
    \"build_script\": {\"executions\": 1, \"elapsed_us\": 500},
}
print(\"fixture stdout\")
print(\"srs-artifact-cache-stats=\" + json.dumps(stats), file=sys.stderr)
""")
            cargo.chmod(0o755)
            cargo_real = root / "cargo-srs-real"
            cargo_real.write_text(
                "#!/usr/bin/env python3\nprint('cargo 1.0.0 (real fixture)')\n"
            )
            cargo_real.chmod(0o755)
            rustc = root / "rustc"
            rustc.write_text(
                "#!/usr/bin/env python3\nprint('rustc 1.0.0 (fixture)')\n"
            )
            rustc.chmod(0o755)
            provenance_record = json.loads(provenance.read_text())
            provenance_record["executables"] = {
                "cargo": benchmark.sha256_file(cargo),
                "cargo-srs-real": benchmark.sha256_file(cargo_real),
                "rustc": benchmark.sha256_file(rustc),
            }
            provenance.write_text(json.dumps(provenance_record))
            config = root / "workloads.json"
            config.write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "workloads": {
                            "sample": {"command": ["build"], "environment": {}}
                        },
                    }
                )
            )
            output = root / "results"
            subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "run",
                    "--workspace",
                    str(subject),
                    "--expect-workspace-rev",
                    subject_revision,
                    "--expect-srs-rev",
                    srs_revision,
                    "--cargo",
                    str(cargo),
                    "--toolchain-provenance",
                    str(provenance),
                    "--config",
                    str(config),
                    "--workloads",
                    "sample",
                    "--trials",
                    "1",
                    "--target-root",
                    str(root / "targets"),
                    "--cache-root",
                    str(root / "caches"),
                    "--output",
                    str(output),
                    "--allow-dirty",
                ],
                check=True,
                stdout=subprocess.PIPE,
                text=True,
            )
            records = [
                json.loads(line)
                for line in (output / "runs.jsonl").read_text().splitlines()
            ]
            self.assertEqual(len(records), 4)
            for record in records:
                run = output / record["relative_directory"]
                self.assertTrue((run / "stdout.log").is_file())
                self.assertTrue((run / "stderr.log").is_file())
                self.assertTrue((run / "result.json").is_file())
                self.assertTrue((run / "stats.json").is_file())
            summary = json.loads((output / "summary.json").read_text())
            warm = next(
                group for group in summary["groups"] if group["state"] == "warm"
            )
            self.assertAlmostEqual(warm["coverage"]["eligibility"], 2 / 3)
            self.assertEqual(warm["coverage"]["effectiveness"], 1.0)
            self.assertAlmostEqual(warm["coverage"]["total_coverage"], 2 / 3)
            self.assertEqual(
                warm["measurements"]["restore"]["logical_bytes"]["median"],
                2048,
            )
            self.assertEqual(
                warm["measurements"]["materialization"]["hardlinked_files"]["median"],
                4,
            )
            self.assertEqual(
                warm["measurements"]["compiler_identity"]["bytes"]["median"],
                4096,
            )
            self.assertEqual(
                warm["measurements"]["action_inputs"]["bytes"]["median"],
                4096,
            )
            self.assertEqual(
                warm["measurements"]["build_script"]["executions"]["median"],
                1,
            )
            self.assertEqual(
                warm["ineligible_by_reason"]["proc_macro"]["total"],
                1,
            )
            markdown = (output / "summary.md").read_text()
            self.assertIn(
                "## Restore, materialization, and publication medians", markdown
            )
            self.assertIn("## Ineligible actions by reason", markdown)

    def test_missing_optional_statistics_remain_unavailable(self) -> None:
        summary = benchmark.summarize_statistics([{"units": {}}])
        self.assertIsNone(summary["measurements"]["restore"]["files"]["median"])
        self.assertIsNone(summary["measurements"]["action_inputs"]["bytes"]["median"])
        self.assertEqual(summary["ineligible_by_reason"], {})

    def test_real_statistics_fixture_matches_summary_contract(self) -> None:
        stats = json.loads(REAL_STATS_FIXTURE.read_text())
        self.assertEqual(stats["version"], benchmark.SCHEMA_VERSION)
        summary = benchmark.summarize_statistics([stats])
        self.assertEqual(
            summary["measurements"]["action_inputs"]["files"]["median"], 0
        )
        self.assertEqual(
            summary["measurements"]["compiler_identity"]["computations"]["median"],
            1,
        )
        self.assertEqual(
            summary["ineligible_by_reason"]["compiler_identity_unavailable"][
                "median"
            ],
            1,
        )


if __name__ == "__main__":
    unittest.main()
