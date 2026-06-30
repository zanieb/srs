#!/usr/bin/env python3
"""Compare runtime performance of matched LLVM and Cranelift binaries."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import itertools
import json
import math
import os
import platform
import random
import re
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence

SCHEMA_VERSION = 3
ROOT_MARKER = ".srs-cranelift-runtime-benchmark-root"
WORKLOADS = ("uv-venv", "uv-lock", "ruff-check", "ty-check")
UV_LOCK_ELAPSED = re.compile(rb"(?<= in )\d+(?:\.\d+)?(?:ms|s)(?=\r?\n|$)")


class BenchmarkError(RuntimeError):
    pass


@dataclass(frozen=True)
class Lane:
    name: str
    uv: Path
    ruff: Path
    ty: Path

    def binary(self, name: str) -> Path:
        return {"uv": self.uv, "ruff": self.ruff, "ty": self.ty}[name]


@dataclass(frozen=True)
class Command:
    argv: tuple[str, ...]
    cwd: Path
    environment: Mapping[str, str]


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def parse_lane(value: str) -> Lane:
    name, separator, paths = value.partition("=")
    binaries = paths.split(",") if separator else []
    if not name or len(binaries) != 3:
        raise argparse.ArgumentTypeError(
            "lanes use NAME=UV_BINARY,RUFF_BINARY,TY_BINARY"
        )
    if any(not path for path in binaries):
        raise argparse.ArgumentTypeError("lane binary paths must not be empty")
    return Lane(name, *(Path(path).expanduser().resolve() for path in binaries))


def balanced_schedule(
    lane_names: Sequence[str], trials: int, seed: int
) -> list[tuple[str, ...]]:
    if trials < 1:
        raise ValueError("trials must be at least one")
    if len(set(lane_names)) != len(lane_names) or len(lane_names) < 2:
        raise ValueError("at least two uniquely named lanes are required")
    permutations = list(itertools.permutations(lane_names))
    rng = random.Random(seed)
    schedule: list[tuple[str, ...]] = []
    while len(schedule) < trials:
        cycle = permutations.copy()
        rng.shuffle(cycle)
        schedule.extend(cycle)
    return schedule[:trials]


def repository_state(repository: Path) -> dict[str, Any]:
    def git(*arguments: str) -> str:
        completed = subprocess.run(
            ["git", "-C", str(repository), *arguments],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        return completed.stdout.strip()

    return {
        "path": str(repository),
        "revision": git("rev-parse", "HEAD"),
        "status": git("status", "--porcelain=v1", "--untracked-files=normal"),
    }


def validate_revision(actual: str, expected: str | None, label: str) -> None:
    if expected is None:
        return
    if len(expected) < 7 or not actual.startswith(expected):
        raise BenchmarkError(
            f"{label} revision {actual} does not match expected {expected!r}"
        )


def prepare_scratch(path: Path) -> None:
    if path.exists():
        if not path.is_dir() or not (path / ROOT_MARKER).is_file():
            raise BenchmarkError(
                f"refusing existing unowned scratch directory {path}; "
                f"expected marker {ROOT_MARKER}"
            )
    else:
        path.mkdir(parents=True)
        (path / ROOT_MARKER).write_text(f"schema={SCHEMA_VERSION}\n")


def benchmark_environment(extra: Mapping[str, str]) -> dict[str, str]:
    environment = os.environ.copy()
    environment.update(
        {
            "CLICOLOR": "0",
            "NO_COLOR": "1",
            "TERM": "dumb",
            "UV_NO_PROGRESS": "1",
        }
    )
    environment.update(extra)
    return environment


def command_for(
    workload: str,
    lane: Lane,
    uv_workspace: Path,
    ruff_workspace: Path,
    scratch: Path,
) -> Command:
    if workload == "uv-venv":
        return Command(
            (
                str(lane.uv),
                "venv",
                "--clear",
                str(scratch / "uv-venv"),
            ),
            uv_workspace,
            benchmark_environment({}),
        )
    if workload == "uv-lock":
        return Command(
            (str(lane.uv), "lock", "--check"),
            uv_workspace,
            benchmark_environment({"UV_NO_CACHE": "1", "UV_OFFLINE": "1"}),
        )
    if workload == "ruff-check":
        return Command(
            (
                str(lane.ruff),
                "check",
                "--isolated",
                "--no-cache",
                "--exit-zero",
                "--quiet",
                "crates/ruff_linter/resources/test/fixtures",
            ),
            ruff_workspace,
            benchmark_environment({}),
        )
    if workload == "ty-check":
        return Command(
            (
                str(lane.ty),
                "check",
                "--project",
                "scripts/ty_benchmark",
            ),
            ruff_workspace,
            benchmark_environment({}),
        )
    raise BenchmarkError(f"unknown workload {workload!r}")


def run_probe(command: Command, workload: str) -> dict[str, Any]:
    completed = subprocess.run(
        command.argv,
        cwd=command.cwd,
        env=command.environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=300,
    )
    stdout = completed.stdout
    stderr = completed.stderr
    if workload == "uv-lock":
        stderr = UV_LOCK_ELAPSED.sub(b"<elapsed>", stderr)
    return {
        "exit_code": completed.returncode,
        "stdout_sha256": hashlib.sha256(stdout).hexdigest(),
        "stderr_sha256": hashlib.sha256(stderr).hexdigest(),
        "raw_stdout_sha256": hashlib.sha256(completed.stdout).hexdigest(),
        "raw_stderr_sha256": hashlib.sha256(completed.stderr).hexdigest(),
    }


def run_timed(command: Command) -> tuple[int, int]:
    started = time.perf_counter_ns()
    completed = subprocess.run(
        command.argv,
        cwd=command.cwd,
        env=command.environment,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=300,
    )
    return completed.returncode, time.perf_counter_ns() - started


def summarize(samples_ns: Sequence[int]) -> dict[str, float]:
    samples_ms = [sample / 1_000_000 for sample in samples_ns]
    return {
        "median_ms": statistics.median(samples_ms),
        "mean_ms": statistics.fmean(samples_ms),
        "stdev_ms": statistics.stdev(samples_ms) if len(samples_ms) > 1 else 0.0,
        "min_ms": min(samples_ms),
        "max_ms": max(samples_ms),
    }


def exact_sign_test(wins: int, losses: int) -> float:
    observations = wins + losses
    if observations == 0:
        return 1.0
    tail = sum(
        math.comb(observations, successes)
        for successes in range(min(wins, losses) + 1)
    )
    return min(1.0, 2 * tail / 2**observations)


def compare_samples(
    reference_ns: Sequence[int], candidate_ns: Sequence[int]
) -> dict[str, Any]:
    if len(reference_ns) != len(candidate_ns) or not reference_ns:
        raise ValueError("paired samples must have the same nonzero length")
    comparisons = list(zip(reference_ns, candidate_ns, strict=True))
    changes = [
        (candidate - reference) / reference * 100
        for reference, candidate in comparisons
    ]
    median_change = statistics.median(changes)
    wins = sum(candidate < reference for reference, candidate in comparisons)
    losses = sum(candidate > reference for reference, candidate in comparisons)
    ties = len(changes) - wins - losses
    return {
        "trials": len(changes),
        "median_change_percent": median_change,
        "median_absolute_deviation_percent": statistics.median(
            abs(change - median_change) for change in changes
        ),
        "wins": wins,
        "losses": losses,
        "ties": ties,
        "two_sided_sign_test_p": exact_sign_test(wins, losses),
    }


def binary_metadata(path: Path) -> dict[str, Any]:
    completed = subprocess.run(
        [str(path), "--version"],
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        timeout=30,
    )
    return {
        "path": str(path),
        "bytes": path.stat().st_size,
        "sha256": sha256_file(path),
        "version": completed.stdout.strip(),
    }


def write_json(path: Path, value: Any, overwrite: bool) -> None:
    if path.exists() and not overwrite:
        raise BenchmarkError(f"output already exists: {path}; pass --overwrite to replace it")
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    temporary.replace(path)


def run(args: argparse.Namespace) -> None:
    lanes = {lane.name: lane for lane in args.lane}
    if len(lanes) != len(args.lane) or len(lanes) < 2:
        raise BenchmarkError("at least two uniquely named --lane values are required")
    if args.reference not in lanes:
        raise BenchmarkError(f"reference lane {args.reference!r} was not provided")

    for lane in lanes.values():
        for binary_name in ("uv", "ruff", "ty"):
            binary = lane.binary(binary_name)
            if not binary.is_file() or not os.access(binary, os.X_OK):
                raise BenchmarkError(f"missing executable {binary_name} for {lane.name}: {binary}")

    uv_workspace = args.uv_workspace.resolve()
    ruff_workspace = args.ruff_workspace.resolve()
    uv_state = repository_state(uv_workspace)
    ruff_state = repository_state(ruff_workspace)
    if not args.allow_dirty:
        for label, state in (("uv", uv_state), ("Ruff", ruff_state)):
            if state["status"]:
                raise BenchmarkError(f"{label} workspace is dirty: {state['path']}")
    validate_revision(uv_state["revision"], args.expect_uv_rev, "uv")
    validate_revision(ruff_state["revision"], args.expect_ruff_rev, "Ruff")

    prepare_scratch(args.scratch.resolve())
    workloads = tuple(dict.fromkeys(args.workloads.split(",")))
    unknown = set(workloads) - set(WORKLOADS)
    if unknown:
        raise BenchmarkError(f"unknown workloads: {', '.join(sorted(unknown))}")

    binary_info = {
        lane_name: {
            binary_name: binary_metadata(lane.binary(binary_name))
            for binary_name in ("uv", "ruff", "ty")
        }
        for lane_name, lane in lanes.items()
    }
    for binary_name in ("uv", "ruff", "ty"):
        versions = {
            metadata[binary_name]["version"] for metadata in binary_info.values()
        }
        if len(versions) != 1:
            raise BenchmarkError(
                f"{binary_name} versions differ between lanes: {sorted(versions)}"
            )

    lane_names = tuple(lanes)
    schedule = balanced_schedule(lane_names, args.runs, args.seed)
    records: list[dict[str, Any]] = []
    probes: dict[str, dict[str, Any]] = {}
    summaries: dict[str, dict[str, Any]] = {}
    pairwise: dict[str, dict[str, Any]] = {}

    for workload in workloads:
        commands = {
            lane_name: command_for(
                workload,
                lane,
                uv_workspace,
                ruff_workspace,
                args.scratch.resolve(),
            )
            for lane_name, lane in lanes.items()
        }
        # `uv venv --clear` reports different setup details when its target does not exist yet.
        # Prime that state outside both the correctness probes and timed region so every lane
        # observes an existing valid environment.
        if workload == "uv-venv":
            prime = run_probe(commands[args.reference], workload)
            if prime["exit_code"] != 0:
                raise BenchmarkError(
                    f"{workload} state priming in {args.reference} exited "
                    f"{prime['exit_code']}"
                )
        probes[workload] = {
            lane_name: run_probe(command, workload) for lane_name, command in commands.items()
        }
        exit_codes = {probe["exit_code"] for probe in probes[workload].values()}
        if len(exit_codes) != 1:
            raise BenchmarkError(
                f"{workload} probe exit codes differ: {probes[workload]}"
            )
        expected_exit = exit_codes.pop()
        output_digests = {
            (probe["stdout_sha256"], probe["stderr_sha256"])
            for probe in probes[workload].values()
        }
        if len(output_digests) != 1:
            raise BenchmarkError(
                f"{workload} probe output differs between lanes: {probes[workload]}"
            )

        warmup_schedule = balanced_schedule(lane_names, max(args.warmups, 1), args.seed + 1)
        for order in warmup_schedule[: args.warmups]:
            for lane_name in order:
                exit_code, _ = run_timed(commands[lane_name])
                if exit_code != expected_exit:
                    raise BenchmarkError(
                        f"{workload} warmup in {lane_name} exited {exit_code}, expected {expected_exit}"
                    )

        samples: dict[str, list[int]] = {lane_name: [] for lane_name in lane_names}
        for trial, order in enumerate(schedule, 1):
            for position, lane_name in enumerate(order, 1):
                exit_code, elapsed_ns = run_timed(commands[lane_name])
                if exit_code != expected_exit:
                    raise BenchmarkError(
                        f"{workload} trial {trial} in {lane_name} exited {exit_code}, "
                        f"expected {expected_exit}"
                    )
                samples[lane_name].append(elapsed_ns)
                records.append(
                    {
                        "workload": workload,
                        "trial": trial,
                        "position": position,
                        "lane": lane_name,
                        "elapsed_ns": elapsed_ns,
                    }
                )

        reference_median = statistics.median(samples[args.reference])
        summaries[workload] = {
            lane_name: {
                **summarize(lane_samples),
                "slowdown_vs_reference": statistics.median(lane_samples)
                / reference_median,
            }
            for lane_name, lane_samples in samples.items()
        }
        pairwise[workload] = {
            f"{candidate}_vs_{reference}": {
                "reference": reference,
                "candidate": candidate,
                **compare_samples(samples[reference], samples[candidate]),
            }
            for reference, candidate in itertools.combinations(lane_names, 2)
        }

    result = {
        "schema": SCHEMA_VERSION,
        "created_at": utc_now(),
        "host": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": sys.version,
        },
        "configuration": {
            "runs": args.runs,
            "warmups": args.warmups,
            "seed": args.seed,
            "reference": args.reference,
            "workloads": workloads,
            "schedule": schedule,
        },
        "repositories": {"uv": uv_state, "ruff": ruff_state},
        "binaries": binary_info,
        "probes": probes,
        "trials": records,
        "summary": summaries,
        "pairwise": pairwise,
    }
    write_json(args.output.resolve(), result, args.overwrite)

    for workload, workload_summary in summaries.items():
        print(workload)
        for lane_name, lane_summary in workload_summary.items():
            print(
                f"  {lane_name}: {lane_summary['median_ms']:.2f} ms median, "
                f"{lane_summary['slowdown_vs_reference']:.2f}x {args.reference}"
            )


def parser() -> argparse.ArgumentParser:
    argument_parser = argparse.ArgumentParser(description=__doc__)
    argument_parser.add_argument(
        "--lane",
        action="append",
        required=True,
        type=parse_lane,
        help="NAME=UV_BINARY,RUFF_BINARY,TY_BINARY; repeat for each lane",
    )
    argument_parser.add_argument("--reference", default="llvm")
    argument_parser.add_argument("--uv-workspace", required=True, type=Path)
    argument_parser.add_argument("--ruff-workspace", required=True, type=Path)
    argument_parser.add_argument("--expect-uv-rev")
    argument_parser.add_argument("--expect-ruff-rev")
    argument_parser.add_argument("--scratch", required=True, type=Path)
    argument_parser.add_argument("--output", required=True, type=Path)
    argument_parser.add_argument("--workloads", default=",".join(WORKLOADS))
    argument_parser.add_argument("--runs", type=int, default=20)
    argument_parser.add_argument("--warmups", type=int, default=5)
    argument_parser.add_argument("--seed", type=int, default=20260618)
    argument_parser.add_argument("--allow-dirty", action="store_true")
    argument_parser.add_argument("--overwrite", action="store_true")
    return argument_parser


def main() -> int:
    try:
        args = parser().parse_args()
        if args.runs < 1 or args.warmups < 0:
            raise BenchmarkError("--runs must be positive and --warmups nonnegative")
        run(args)
    except (BenchmarkError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
