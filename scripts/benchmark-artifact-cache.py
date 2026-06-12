#!/usr/bin/env python3
"""Run and summarize controlled SRS Cargo artifact-cache benchmarks."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import itertools
import json
import os
import platform
import random
import resource
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence

SCHEMA_VERSION = 1
STATS_PREFIX = "srs-artifact-cache-stats="
ROOT_MARKER = ".srs-artifact-cache-benchmark-root"
STATES = ("disabled", "cold", "warm")
SUMMARY_METRICS = {
    "restore": {
        "files": ("restore", "files"),
        "logical_bytes": ("restore", "logical_bytes"),
    },
    "materialization": {
        "hardlinked_files": ("materialization", "hardlinked_files"),
        "hardlinked_logical_bytes": (
            "materialization",
            "hardlinked_logical_bytes",
        ),
        "copied_files": ("materialization", "copied_files"),
        "copied_logical_bytes": ("materialization", "copied_logical_bytes"),
        "cross_device_copied_files": (
            "materialization",
            "cross_device_copied_files",
        ),
        "cross_device_copied_logical_bytes": (
            "materialization",
            "cross_device_copied_logical_bytes",
        ),
        "elapsed_us": ("materialization", "elapsed_us"),
    },
    "compiler_identity": {
        "computations": ("hashing", "compiler_identity", "computations"),
        "reuses": ("hashing", "compiler_identity", "reuses"),
        "files": ("hashing", "compiler_identity", "files"),
        "bytes": ("hashing", "compiler_identity", "bytes"),
        "wall_us": ("hashing", "compiler_identity", "wall_us"),
        "computation_wall_us": (
            "hashing",
            "compiler_identity",
            "computation_wall_us",
        ),
        "computation_cpu_us": (
            "hashing",
            "compiler_identity",
            "computation_cpu_us",
        ),
    },
    "action_inputs": {
        "calls": ("hashing", "action_inputs", "calls"),
        # These are optional so the harness remains compatible with statistics
        # schema v1 binaries that predate input-volume counters.
        "files": ("hashing", "action_inputs", "files"),
        "bytes": ("hashing", "action_inputs", "bytes"),
        "wall_us": ("hashing", "action_inputs", "wall_us"),
    },
    "lookup": {
        "elapsed_us": ("lookup", "elapsed_us"),
    },
    "preflight": {
        "elapsed_us": ("preflight", "elapsed_us"),
    },
    "publication": {
        "attempts": ("publication", "attempts"),
        "stored": ("publication", "stored"),
        "files": ("publication", "files"),
        "logical_bytes": ("publication", "logical_bytes"),
        "elapsed_us": ("publication", "elapsed_us"),
    },
    "rustc": {
        "elapsed_us": ("rustc", "elapsed_us"),
    },
    "build_script": {
        "executions": ("build_script", "executions"),
        "elapsed_us": ("build_script", "elapsed_us"),
    },
}
SELECTED_ENVIRONMENT = (
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_BUILD_JOBS",
    "CARGO_INCREMENTAL",
    "CARGO_TARGET_DIR",
    "CARGO_TERM_COLOR",
    "CARGO_UNSTABLE_ARTIFACT_CACHE",
    "PATH",
    "CARGO_BUILD_ARTIFACT_CACHE_DIR",
    "CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION",
    "CARGO_BUILD_ARTIFACT_CACHE_MAX_SIZE",
    "SRS_CARGO_ARTIFACT_CACHE",
    "SRS_CARGO_ARTIFACT_CACHE_DIR",
    "SRS_CARGO_ARTIFACT_CACHE_MATERIALIZATION",
    "SRS_CARGO_ARTIFACT_CACHE_MAX_SIZE",
    "SRS_CARGO_ARTIFACT_CACHE_STATS",
    "SRS_TARGET_CODEGEN_BACKEND",
    "SLD_INCREMENTAL",
)


class BenchmarkError(RuntimeError):
    pass


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    temporary.replace(path)


def read_json(path: Path) -> Any:
    return json.loads(path.read_text())


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def balanced_schedule(trials: int, seed: int) -> list[tuple[str, str, str]]:
    """Return randomized blocks balanced over every six consecutive trials."""
    if trials < 1:
        raise ValueError("trials must be at least one")
    rng = random.Random(seed)
    permutations = list(itertools.permutations(STATES))
    schedule: list[tuple[str, str, str]] = []
    while len(schedule) < trials:
        cycle = permutations.copy()
        rng.shuffle(cycle)
        schedule.extend(cycle)
    return schedule[:trials]


def git_output(repository: Path, *arguments: str) -> str:
    try:
        completed = subprocess.run(
            ["git", "-C", str(repository), *arguments],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        raise BenchmarkError(
            f"failed to inspect Git repository {repository}: {error}"
        ) from error
    return completed.stdout.strip()


def revision_matches(actual: str, expected: str) -> bool:
    expected = expected.lower()
    actual = actual.lower()
    return len(expected) >= 7 and actual.startswith(expected)


def check_repository(
    repository: Path, expected_revision: str, *, allow_dirty: bool
) -> dict[str, Any]:
    actual = git_output(repository, "rev-parse", "HEAD")
    if not revision_matches(actual, expected_revision):
        raise BenchmarkError(
            f"{repository} is at {actual}, expected revision {expected_revision}"
        )
    status = repository_status(repository)
    dirty = bool(status)
    if dirty and not allow_dirty:
        raise BenchmarkError(f"refusing dirty benchmark repository: {repository}")
    return {"path": str(repository), "revision": actual, "dirty": dirty}


def repository_status(repository: Path) -> str:
    return git_output(
        repository,
        "status",
        "--porcelain=v1",
        "--untracked-files=all",
        "--ignored=matching",
    )


def verify_repository_status(repository: Path, expected: str) -> None:
    actual = repository_status(repository)
    if actual != expected:
        raise BenchmarkError(f"benchmark workload mutated repository state: {repository}")


def load_workloads(path: Path) -> dict[str, dict[str, Any]]:
    value = read_json(path)
    if value.get("schema_version") != SCHEMA_VERSION:
        raise BenchmarkError(f"unsupported workload schema in {path}")
    workloads = value.get("workloads")
    if not isinstance(workloads, dict) or not workloads:
        raise BenchmarkError(f"no workloads configured in {path}")
    for name, workload in workloads.items():
        command = workload.get("command")
        environment = workload.get("environment", {})
        if (
            not isinstance(command, list)
            or not command
            or not all(isinstance(argument, str) for argument in command)
        ):
            raise BenchmarkError(f"workload {name!r} has an invalid command")
        if not isinstance(environment, dict) or not all(
            isinstance(key, str) and isinstance(item, str)
            for key, item in environment.items()
        ):
            raise BenchmarkError(f"workload {name!r} has an invalid environment")
    return workloads


def validate_backend_workloads(backend: str, selected: Sequence[str]) -> None:
    if backend != "llvm" and "test" in selected:
        raise BenchmarkError(
            "the pinned uv nextest workload requires LLVM; run Cranelift only for "
            "the build and Clippy workloads"
        )


def resolve_program(program: str) -> Path:
    candidate = Path(program).expanduser()
    if candidate.parent != Path(".") or candidate.is_absolute():
        resolved = candidate.resolve()
        if not resolved.is_file() or not os.access(resolved, os.X_OK):
            raise BenchmarkError(f"Cargo executable is not executable: {resolved}")
        return resolved
    found = shutil.which(program)
    if found is None:
        raise BenchmarkError(f"Cargo executable was not found: {program}")
    return Path(found).resolve()


def command_output(command: Sequence[str]) -> str:
    try:
        completed = subprocess.run(
            command,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        raise BenchmarkError(f"failed to run {' '.join(command)}: {error}") from error
    return completed.stdout.strip()


def program_metadata(program: str | Path, version_arguments: Sequence[str]) -> dict[str, Any]:
    resolved = resolve_program(str(program))
    return {
        "path": str(resolved),
        "sha256": sha256_file(resolved),
        "version": command_output([str(resolved), *version_arguments]),
    }


def load_toolchain_provenance(path: Path, srs_revision: str) -> dict[str, Any]:
    value = read_json(path)
    if not isinstance(value, dict):
        raise BenchmarkError("toolchain provenance must be a JSON object")
    if value.get("srs_revision") != srs_revision:
        raise BenchmarkError(
            "toolchain provenance SRS revision does not match the benchmark checkout"
        )
    artifact_digest = value.get("artifact_sha256")
    if (
        not isinstance(artifact_digest, str)
        or len(artifact_digest) != 64
        or any(character not in "0123456789abcdef" for character in artifact_digest.lower())
    ):
        raise BenchmarkError("toolchain provenance has an invalid artifact_sha256")
    if not isinstance(value.get("source"), str) or not value["source"]:
        raise BenchmarkError("toolchain provenance must identify its source")
    if not isinstance(value.get("executables"), dict):
        raise BenchmarkError("toolchain provenance must include executable digests")
    return value


def validate_toolchain_executables(
    provenance: Mapping[str, Any],
    cargo: Mapping[str, Any],
    rustc: Mapping[str, Any] | None,
) -> None:
    actual = {"cargo": cargo["sha256"]}
    if isinstance(cargo.get("real"), Mapping):
        actual["cargo-srs-real"] = cargo["real"]["sha256"]
    if rustc is not None:
        actual["rustc"] = rustc["sha256"]
    expected = provenance["executables"]
    required = {"cargo", "cargo-srs-real", "rustc"}
    if set(actual) != required or set(expected) != required:
        raise BenchmarkError(
            "toolchain provenance must bind cargo, cargo-srs-real, and rustc"
        )
    if expected != actual:
        raise BenchmarkError(
            "installed Cargo/rustc digests do not match toolchain provenance"
        )


def ensure_safe_root(path: Path, forbidden: Iterable[Path]) -> Path:
    resolved = path.expanduser().resolve()
    if resolved == Path(resolved.anchor):
        raise BenchmarkError(f"refusing filesystem root as benchmark path: {resolved}")
    for item in forbidden:
        forbidden_path = item.resolve()
        if (
            resolved == forbidden_path
            or forbidden_path in resolved.parents
            or resolved in forbidden_path.parents
        ):
            raise BenchmarkError(
                f"benchmark path {resolved} contains protected path {forbidden_path}"
            )
    return resolved


def validate_root_for_prepare(path: Path, *, overwrite: bool) -> None:
    if not path.exists():
        return
    if not path.is_dir():
        raise BenchmarkError(f"benchmark path is not a directory: {path}")
    entries = list(path.iterdir())
    if not entries:
        return
    if not overwrite:
        raise BenchmarkError(f"benchmark path is not empty: {path}; use --overwrite")
    marker = path / ROOT_MARKER
    if not marker.is_file() or marker.is_symlink():
        raise BenchmarkError(
            f"refusing to overwrite unowned benchmark path without {ROOT_MARKER}: {path}"
        )


def prepare_root(path: Path, *, overwrite: bool) -> None:
    validate_root_for_prepare(path, overwrite=overwrite)
    path.mkdir(parents=True, exist_ok=True)
    if overwrite:
        for entry in path.iterdir():
            if entry.name == ROOT_MARKER:
                continue
            remove_tree(entry)
    (path / ROOT_MARKER).write_text(f"schema={SCHEMA_VERSION}\n")


def validate_independent_roots(paths: Sequence[Path]) -> None:
    if len(set(paths)) != len(paths):
        raise BenchmarkError("benchmark roots must be different")
    for left, right in itertools.permutations(paths, 2):
        if left in right.parents:
            raise BenchmarkError(
                f"benchmark roots must not contain each other: {left}, {right}"
            )


def remove_tree(path: Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink(missing_ok=True)
    elif path.exists():
        shutil.rmtree(path)


def directory_totals(path: Path) -> dict[str, int]:
    files = 0
    logical_bytes = 0
    if not path.exists():
        return {"files": 0, "logical_bytes": 0}
    for root, _, names in os.walk(path):
        for name in names:
            candidate = Path(root) / name
            try:
                stat = candidate.lstat()
            except FileNotFoundError:
                continue
            files += 1
            logical_bytes += stat.st_size
    return {"files": files, "logical_bytes": logical_bytes}


def tree_digest(path: Path) -> dict[str, Any]:
    digest = hashlib.sha256()
    totals = directory_totals(path)
    for root, directories, files in os.walk(path):
        directories.sort()
        files.sort()
        root_path = Path(root)
        for name in files:
            candidate = root_path / name
            relative = candidate.relative_to(path).as_posix().encode()
            stat = candidate.lstat()
            digest.update(len(relative).to_bytes(8, "little"))
            digest.update(relative)
            digest.update(stat.st_mode.to_bytes(8, "little"))
            if candidate.is_symlink():
                digest.update(b"L")
                digest.update(os.readlink(candidate).encode())
            else:
                digest.update(b"F")
                with candidate.open("rb") as stream:
                    while chunk := stream.read(1024 * 1024):
                        digest.update(chunk)
    return {**totals, "sha256": digest.hexdigest()}


def reset_active_cache(seed: Path, active: Path) -> None:
    """Copy the immutable population seed into a fresh active cache."""
    if not seed.is_dir():
        raise BenchmarkError(f"warm seed does not exist: {seed}")
    remove_tree(active)
    shutil.copytree(seed, active, symlinks=True, copy_function=shutil.copy2)


def read_proc_file(path: str) -> str | None:
    try:
        return Path(path).read_text()
    except OSError:
        return None


def system_sample() -> dict[str, Any]:
    load_average = None
    try:
        load_average = os.getloadavg()
    except OSError:
        pass
    memory = {}
    meminfo = read_proc_file("/proc/meminfo")
    if meminfo:
        for line in meminfo.splitlines():
            key, _, value = line.partition(":")
            if key in {"MemTotal", "MemAvailable", "SwapTotal", "SwapFree"}:
                memory[key] = value.strip()
    pressure = {
        name: read_proc_file(f"/proc/pressure/{name}")
        for name in ("cpu", "io", "memory")
    }
    return {
        "at": utc_now(),
        "monotonic_ns": time.monotonic_ns(),
        "load_average": load_average,
        "memory": memory,
        "pressure": pressure,
    }


def filesystem_metadata(path: Path) -> dict[str, Any]:
    stat = path.stat()
    statvfs = os.statvfs(path)
    filesystem_type = None
    mount_point = None
    mounts = read_proc_file("/proc/mounts")
    if mounts:
        resolved = path.resolve()
        candidates = []
        for line in mounts.splitlines():
            fields = line.split()
            if len(fields) < 3:
                continue
            mounted = Path(fields[1].replace("\\040", " "))
            if resolved == mounted or mounted in resolved.parents:
                candidates.append((len(mounted.parts), str(mounted), fields[2]))
        if candidates:
            _, mount_point, filesystem_type = max(candidates)
    return {
        "path": str(path),
        "device": stat.st_dev,
        "type": filesystem_type,
        "mount_point": mount_point,
        "block_size": statvfs.f_frsize,
        "blocks": statvfs.f_blocks,
        "available_blocks": statvfs.f_bavail,
    }


def host_metadata() -> dict[str, Any]:
    affinity = None
    if hasattr(os, "sched_getaffinity"):
        affinity = sorted(os.sched_getaffinity(0))
    cpu_model = None
    cpuinfo = read_proc_file("/proc/cpuinfo")
    if cpuinfo:
        for line in cpuinfo.splitlines():
            key, _, value = line.partition(":")
            if key.strip() in {"model name", "Hardware"}:
                cpu_model = value.strip()
                break
    return {
        "platform": platform.platform(),
        "uname": list(platform.uname()),
        "python": platform.python_version(),
        "cpu_count": os.cpu_count(),
        "cpu_model": cpu_model,
        "cpu_affinity": affinity,
        "cgroup": read_proc_file("/proc/self/cgroup"),
        "mounts": read_proc_file("/proc/mounts"),
    }


def extract_stats(stderr_path: Path) -> dict[str, Any]:
    records = []
    with stderr_path.open(errors="replace") as stream:
        for line in stream:
            position = line.find(STATS_PREFIX)
            if position == -1:
                continue
            payload = line[position + len(STATS_PREFIX) :].strip()
            try:
                records.append(json.loads(payload))
            except json.JSONDecodeError as error:
                raise BenchmarkError(
                    f"invalid artifact-cache statistics in {stderr_path}: {error}"
                ) from error
    if len(records) != 1:
        raise BenchmarkError(
            f"expected one artifact-cache statistics record in {stderr_path}, found {len(records)}"
        )
    if records[0].get("version") != SCHEMA_VERSION:
        raise BenchmarkError(
            f"unsupported artifact-cache statistics version in {stderr_path}"
        )
    return records[0]


def validate_stats_state(
    stats: Mapping[str, Any], *, workload: str, state: str
) -> None:
    expected_configured = state != "disabled"
    if stats.get("configured") is not expected_configured:
        raise BenchmarkError(
            f"{workload} {state} reported configured={stats.get('configured')!r}, "
            f"expected {expected_configured}"
        )


def formatted_environment(
    configured: Mapping[str, str], *, workspace: Path, run_directory: Path
) -> dict[str, str]:
    return {
        key: value.format(
            workspace=str(workspace), run_directory=str(run_directory)
        )
        for key, value in configured.items()
    }


def benchmark_environment(
    base: Mapping[str, str],
    workload_environment: Mapping[str, str],
    *,
    state: str,
    active_cache: Path,
    target: Path,
    workspace: Path,
    backend: str,
    linker: str | None,
    jobs: int,
    materialization: str,
    run_directory: Path,
    toolchain_bin: Path,
) -> dict[str, str]:
    environment = dict(base)
    for key in (
        "CARGO_BUILD_ARTIFACT_CACHE_DIR",
        "CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION",
        "CARGO_BUILD_ARTIFACT_CACHE_MAX_SIZE",
        "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_UNSTABLE_ARTIFACT_CACHE",
        "RUSTFLAGS",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "SRS_CARGO_ARTIFACT_CACHE",
        "SRS_CARGO_ARTIFACT_CACHE_DIR",
        "SRS_CARGO_ARTIFACT_CACHE_MATERIALIZATION",
        "SRS_CARGO_ARTIFACT_CACHE_MAX_SIZE",
        "SRS_ENCODED_TARGET_RUSTFLAGS",
    ):
        environment.pop(key, None)
    environment.update(
        formatted_environment(
            workload_environment,
            workspace=workspace,
            run_directory=run_directory,
        )
    )
    # This matrix isolates the portable artifact cache. Exact-path target
    # snapshots are a separate benchmark layer and must not leak in from the
    # caller or a workload configuration.
    for key in (
        "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "RUSTC",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "SRS_CARGO_ARTIFACT_CACHE_SNAPSHOT_MANIFEST",
        "SRS_CARGO_ARTIFACT_CACHE_SNAPSHOT_RESTORE_MANIFEST",
    ):
        environment.pop(key, None)
    environment.update(
        {
            "CARGO_BUILD_JOBS": str(jobs),
            "CARGO_INCREMENTAL": "0",
            "CARGO_TARGET_DIR": str(target),
            "CARGO_TERM_COLOR": "never",
            "PATH": str(toolchain_bin)
            + os.pathsep
            + environment.get("PATH", os.defpath),
            "SRS_CARGO_ARTIFACT_CACHE_STATS": "1",
            "CARGO_BUILD_ARTIFACT_CACHE_MATERIALIZATION": materialization,
            "SRS_CARGO_ARTIFACT_CACHE_MATERIALIZATION": materialization,
            "SRS_TARGET_CODEGEN_BACKEND": backend,
            "SLD_INCREMENTAL": "0",
        }
    )
    flags = [f"-Zcodegen-backend={backend}"]
    if linker:
        flags.append(f"-Clinker={linker}")
    # Host artifacts use the wrapper's explicit [host] configuration because
    # target-applies-to-host is false. Encoded rustflags therefore select the
    # backend and linker only for target artifacts on Linux.
    environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
    if state == "disabled":
        environment["SRS_CARGO_ARTIFACT_CACHE"] = "0"
        environment["CARGO_UNSTABLE_ARTIFACT_CACHE"] = "false"
    else:
        environment.update(
            {
                "CARGO_BUILD_ARTIFACT_CACHE_DIR": str(active_cache),
                "CARGO_UNSTABLE_ARTIFACT_CACHE": "true",
                "SRS_CARGO_ARTIFACT_CACHE": "1",
                "SRS_CARGO_ARTIFACT_CACHE_DIR": str(active_cache),
            }
        )
    return environment


def usage_snapshot() -> dict[str, Any]:
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    return {
        "user_seconds": usage.ru_utime,
        "system_seconds": usage.ru_stime,
        "minor_faults": usage.ru_minflt,
        "major_faults": usage.ru_majflt,
        "voluntary_context_switches": usage.ru_nvcsw,
        "involuntary_context_switches": usage.ru_nivcsw,
    }


def usage_delta(before: Mapping[str, Any], after: Mapping[str, Any]) -> dict[str, Any]:
    result = {}
    for key in before:
        result[key] = after[key] - before[key]
    return result


def recorded_environment(environment: Mapping[str, str]) -> dict[str, str]:
    prefixes = ("INSTA_", "UV_")
    keys = {
        key
        for key in environment
        if key in SELECTED_ENVIRONMENT
        or key == "RUST_BACKTRACE"
        or key.startswith(prefixes)
    }
    return {key: environment[key] for key in sorted(keys)}


def run_invocation(
    *,
    cargo: Path,
    arguments: Sequence[str],
    workspace: Path,
    environment: Mapping[str, str],
    run_directory: Path,
    workload: str,
    state: str,
    trial: int,
    order: int,
    target: Path,
    active_cache: Path,
    population: bool = False,
) -> dict[str, Any]:
    run_directory.mkdir(parents=True, exist_ok=False)
    command = [str(cargo), *arguments]
    write_json(run_directory / "system-before.json", system_sample())
    stdout_path = run_directory / "stdout.log"
    stderr_path = run_directory / "stderr.log"
    started_at = utc_now()
    started_ns = time.monotonic_ns()
    usage_before = usage_snapshot()
    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        try:
            completed = subprocess.run(
                command,
                cwd=workspace,
                env=dict(environment),
                stdout=stdout,
                stderr=stderr,
                check=False,
            )
            returncode = completed.returncode
        except OSError as error:
            returncode = 127
            stderr.write(f"failed to execute benchmark command: {error}\n".encode())
    usage_after = usage_snapshot()
    finished_ns = time.monotonic_ns()
    finished_at = utc_now()
    write_json(run_directory / "system-after.json", system_sample())
    stats_error = None
    stats = None
    try:
        stats = extract_stats(stderr_path)
        validate_stats_state(stats, workload=workload, state=state)
        write_json(run_directory / "stats.json", stats)
    except BenchmarkError as error:
        stats_error = str(error)
    result = {
        "schema_version": SCHEMA_VERSION,
        "workload": workload,
        "state": state,
        "population": population,
        "trial": trial,
        "order": order,
        "command": command,
        "cwd": str(workspace),
        "started_at": started_at,
        "finished_at": finished_at,
        "wall_ns": finished_ns - started_ns,
        "returncode": returncode,
        "resource_usage": usage_delta(usage_before, usage_after),
        "target": {"path": str(target), **directory_totals(target)},
        "artifact_cache": {
            "path": str(active_cache),
            **directory_totals(active_cache),
        },
        "environment": recorded_environment(environment),
        "stats_error": stats_error,
    }
    write_json(run_directory / "result.json", result)
    if returncode != 0:
        raise BenchmarkError(
            f"{workload} {state} trial {trial} failed with exit code {returncode}; "
            f"see {run_directory}"
        )
    if stats_error:
        raise BenchmarkError(stats_error)
    assert stats is not None
    return result


def append_json_line(path: Path, value: Any) -> None:
    with path.open("a") as stream:
        stream.write(json.dumps(value, sort_keys=True) + "\n")


def coverage_metrics(stats: Mapping[str, Any]) -> dict[str, float | int | None]:
    units = stats["units"]
    lookup = stats["lookup"]
    eligible = int(units["eligible"])
    ineligible = int(units["ineligible"])
    hits = int(lookup["hits"])
    total = eligible + ineligible
    return {
        "eligible": eligible,
        "ineligible": ineligible,
        "total": total,
        "hits": hits,
        "rustc_executions": int(stats["rustc"]["executions"]),
        "eligibility": eligible / total if total else None,
        "effectiveness": hits / eligible if eligible else None,
        "total_coverage": hits / total if total else None,
    }


def median_absolute_deviation(values: Sequence[float]) -> float:
    if not values:
        raise ValueError("cannot calculate MAD of an empty sequence")
    center = statistics.median(values)
    return statistics.median(abs(value - center) for value in values)


def nested_number(value: Mapping[str, Any], path: Sequence[str]) -> int | float | None:
    current: Any = value
    for component in path:
        if not isinstance(current, Mapping) or component not in current:
            return None
        current = current[component]
    if isinstance(current, bool) or not isinstance(current, (int, float)):
        return None
    return current


def summarize_optional_samples(
    samples: Sequence[int | float | None],
) -> dict[str, Any]:
    available = [value for value in samples if value is not None]
    return {
        "samples": list(samples),
        "available": len(available),
        "median": statistics.median(available) if available else None,
        "total": sum(available) if available else None,
    }


def summarize_statistics(records: Sequence[Mapping[str, Any]]) -> dict[str, Any]:
    measurements = {}
    for section, fields in SUMMARY_METRICS.items():
        measurements[section] = {
            name: summarize_optional_samples(
                [nested_number(stats, path) for stats in records]
            )
            for name, path in fields.items()
        }

    reasons = set()
    for stats in records:
        by_reason = stats.get("units", {}).get("ineligible_by_reason")
        if isinstance(by_reason, Mapping):
            reasons.update(by_reason)
    ineligible_by_reason = {}
    for reason in sorted(reasons):
        samples = []
        for stats in records:
            by_reason = stats.get("units", {}).get("ineligible_by_reason")
            if not isinstance(by_reason, Mapping):
                samples.append(None)
                continue
            value = by_reason.get(reason, 0)
            samples.append(value if isinstance(value, (int, float)) else None)
        ineligible_by_reason[reason] = summarize_optional_samples(samples)
    return {
        "measurements": measurements,
        "ineligible_by_reason": ineligible_by_reason,
    }


def validate_run_manifest(
    metadata: Mapping[str, Any], results: Sequence[Mapping[str, Any]]
) -> None:
    workloads = metadata.get("workloads")
    schedule = metadata.get("schedule")
    trials = metadata.get("trials")
    if (
        not isinstance(workloads, list)
        or not all(isinstance(item, str) for item in workloads)
        or not isinstance(trials, int)
        or trials < 1
        or not isinstance(schedule, list)
        or len(schedule) != trials
    ):
        raise BenchmarkError("benchmark metadata has an invalid workload schedule")
    expected = set()
    for workload in workloads:
        expected.add((workload, 0, "cold", 0, True))
        for trial, states in enumerate(schedule, 1):
            if not isinstance(states, list) or set(states) != set(STATES):
                raise BenchmarkError("benchmark metadata has an invalid trial block")
            for order, state in enumerate(states, 1):
                expected.add((workload, trial, state, order, False))
    actual = []
    directories = []
    for result in results:
        actual.append(
            (
                result.get("workload"),
                result.get("trial"),
                result.get("state"),
                result.get("order"),
                bool(result.get("population")),
            )
        )
        directories.append(result.get("relative_directory"))
    if len(set(actual)) != len(actual):
        raise BenchmarkError("run manifest contains duplicate workload trials")
    if len(set(directories)) != len(directories) or None in directories:
        raise BenchmarkError("run manifest contains duplicate or missing directories")
    missing = expected.difference(actual)
    unexpected = set(actual).difference(expected)
    if missing or unexpected:
        raise BenchmarkError(
            "run manifest is incomplete or inconsistent "
            f"(missing={len(missing)}, unexpected={len(unexpected)})"
        )


def summarize_records(output: Path) -> dict[str, Any]:
    manifest = output / "runs.jsonl"
    if not manifest.is_file():
        raise BenchmarkError(f"run manifest does not exist: {manifest}")
    metadata_path = output / "metadata.json"
    if not metadata_path.is_file():
        raise BenchmarkError(f"benchmark metadata does not exist: {metadata_path}")
    metadata = read_json(metadata_path)
    results = [json.loads(line) for line in manifest.read_text().splitlines()]
    validate_run_manifest(metadata, results)
    groups: dict[tuple[str, str], list[tuple[dict[str, Any], dict[str, Any]]]] = {}
    by_trial: dict[tuple[str, int], dict[str, dict[str, Any]]] = {}
    for result in results:
        if result.get("population"):
            continue
        run_directory = output / result["relative_directory"]
        stats = read_json(run_directory / "stats.json")
        groups.setdefault((result["workload"], result["state"]), []).append(
            (result, stats)
        )
        by_trial.setdefault((result["workload"], result["trial"]), {})[
            result["state"]
        ] = result
    summary_groups = []
    for (workload, state), records in sorted(groups.items()):
        walls = [result["wall_ns"] / 1_000_000_000 for result, _ in records]
        metrics = [coverage_metrics(stats) for _, stats in records]
        eligible = sum(int(metric["eligible"]) for metric in metrics)
        ineligible = sum(int(metric["ineligible"]) for metric in metrics)
        hits = sum(int(metric["hits"]) for metric in metrics)
        total = eligible + ineligible
        rustc_values = [int(metric["rustc_executions"]) for metric in metrics]
        numeric_metrics = {
            "eligible": eligible,
            "ineligible": ineligible,
            "total": total,
            "hits": hits,
            "rustc_executions": statistics.median(rustc_values),
            "eligibility": eligible / total if total else None,
            "effectiveness": hits / eligible if eligible else None,
            "total_coverage": hits / total if total else None,
        }
        group_summary = {
            "workload": workload,
            "state": state,
            "trials": len(records),
            "wall_seconds": {
                "samples": walls,
                "median": statistics.median(walls),
                "mad": median_absolute_deviation(walls),
            },
            "coverage": numeric_metrics,
        }
        group_summary.update(summarize_statistics([stats for _, stats in records]))
        summary_groups.append(group_summary)
    comparisons = []
    for (workload, trial), states in sorted(by_trial.items()):
        disabled = states["disabled"]["wall_ns"] / 1_000_000_000
        for state in ("cold", "warm"):
            candidate = states[state]["wall_ns"] / 1_000_000_000
            comparisons.append(
                {
                    "workload": workload,
                    "trial": trial,
                    "state": state,
                    "wall_seconds_delta_from_disabled": candidate - disabled,
                }
            )
    paired = []
    for workload in metadata["workloads"]:
        for state in ("cold", "warm"):
            values = [
                item["wall_seconds_delta_from_disabled"]
                for item in comparisons
                if item["workload"] == workload and item["state"] == state
            ]
            paired.append(
                {
                    "workload": workload,
                    "state": state,
                    "samples": values,
                    "median_seconds_delta_from_disabled": statistics.median(values),
                    "mad_seconds": median_absolute_deviation(values),
                }
            )
    return {
        "schema_version": SCHEMA_VERSION,
        "groups": summary_groups,
        "paired_comparisons": paired,
    }


def percentage(value: float | None) -> str:
    return "n/a" if value is None else f"{value * 100:.1f}%"


def measurement_median(
    group: Mapping[str, Any], section: str, field: str
) -> int | float | None:
    return group["measurements"][section][field]["median"]


def format_count(value: int | float | None) -> str:
    if value is None:
        return "n/a"
    if float(value).is_integer():
        return str(int(value))
    return f"{value:.1f}"


def format_bytes(value: int | float | None) -> str:
    if value is None:
        return "n/a"
    units = ("B", "KiB", "MiB", "GiB", "TiB")
    scaled = float(value)
    for unit in units:
        if abs(scaled) < 1024 or unit == units[-1]:
            return f"{scaled:.1f} {unit}"
        scaled /= 1024
    raise AssertionError("unreachable")


def format_milliseconds(value: int | float | None) -> str:
    return "n/a" if value is None else f"{value / 1000:.1f}ms"


def markdown_summary(summary: Mapping[str, Any]) -> str:
    lines = [
        "# Artifact cache benchmark summary",
        "",
        "| Workload | State | n | Wall median | MAD | Aggregate eligible / total | Eligibility | Aggregate hits / eligible | Effectiveness | Aggregate hits / total | Total coverage | rustc median |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for group in summary["groups"]:
        coverage = group["coverage"]

        def count(name: str) -> str:
            value = coverage[name]
            return "n/a" if value is None else str(int(value))

        lines.append(
            "| {workload} | {state} | {trials} | {median:.3f}s | {mad:.3f}s | "
            "{eligible} / {total} | {eligibility} | {hits} / {eligible} | "
            "{effectiveness} | {hits} / {total} | {total_coverage} | {rustc} |".format(
                workload=group["workload"],
                state=group["state"],
                trials=group["trials"],
                median=group["wall_seconds"]["median"],
                mad=group["wall_seconds"]["mad"],
                eligible=count("eligible"),
                total=count("total"),
                hits=count("hits"),
                eligibility=percentage(coverage["eligibility"]),
                effectiveness=percentage(coverage["effectiveness"]),
                total_coverage=percentage(coverage["total_coverage"]),
                rustc=format_count(coverage["rustc_executions"]),
            )
        )
    lines.extend(
        [
            "",
            "## Paired wall-time deltas",
            "",
            "| Workload | State | n | Median delta from disabled | MAD |",
            "| --- | --- | ---: | ---: | ---: |",
        ]
    )
    for comparison in summary["paired_comparisons"]:
        lines.append(
            "| {workload} | {state} | {trials} | {median:+.3f}s | {mad:.3f}s |".format(
                workload=comparison["workload"],
                state=comparison["state"],
                trials=len(comparison["samples"]),
                median=comparison["median_seconds_delta_from_disabled"],
                mad=comparison["mad_seconds"],
            )
        )
    lines.extend(
        [
            "",
            "## Restore, materialization, and publication medians",
            "",
            "| Workload | State | Restored files | Restored bytes | Hardlinked files | Hardlinked bytes | Copied files | Copied bytes | Cross-device files | Cross-device bytes | Published files | Published bytes |",
            "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
        ]
    )
    for group in summary["groups"]:

        def median(section: str, field: str) -> int | float | None:
            return measurement_median(group, section, field)

        lines.append(
            "| {workload} | {state} | {restored_files} | {restored_bytes} | "
            "{hardlinked_files} | {hardlinked_bytes} | {copied_files} | "
            "{copied_bytes} | {cross_files} | {cross_bytes} | {published_files} | "
            "{published_bytes} |".format(
                workload=group["workload"],
                state=group["state"],
                restored_files=format_count(median("restore", "files")),
                restored_bytes=format_bytes(median("restore", "logical_bytes")),
                hardlinked_files=format_count(
                    median("materialization", "hardlinked_files")
                ),
                hardlinked_bytes=format_bytes(
                    median("materialization", "hardlinked_logical_bytes")
                ),
                copied_files=format_count(median("materialization", "copied_files")),
                copied_bytes=format_bytes(
                    median("materialization", "copied_logical_bytes")
                ),
                cross_files=format_count(
                    median("materialization", "cross_device_copied_files")
                ),
                cross_bytes=format_bytes(
                    median("materialization", "cross_device_copied_logical_bytes")
                ),
                published_files=format_count(median("publication", "files")),
                published_bytes=format_bytes(median("publication", "logical_bytes")),
            )
        )

    lines.extend(
        [
            "",
            "## Hashing medians",
            "",
            "| Workload | State | Compiler files | Compiler bytes | Compiler wall | Action calls | Action files | Action bytes | Action wall |",
            "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
        ]
    )
    for group in summary["groups"]:

        def median(section: str, field: str) -> int | float | None:
            return measurement_median(group, section, field)

        lines.append(
            "| {workload} | {state} | {compiler_files} | {compiler_bytes} | "
            "{compiler_wall} | {action_calls} | {action_files} | {action_bytes} | "
            "{action_wall} |".format(
                workload=group["workload"],
                state=group["state"],
                compiler_files=format_count(median("compiler_identity", "files")),
                compiler_bytes=format_bytes(median("compiler_identity", "bytes")),
                compiler_wall=format_milliseconds(
                    median("compiler_identity", "computation_wall_us")
                ),
                action_calls=format_count(median("action_inputs", "calls")),
                action_files=format_count(median("action_inputs", "files")),
                action_bytes=format_bytes(median("action_inputs", "bytes")),
                action_wall=format_milliseconds(median("action_inputs", "wall_us")),
            )
        )

    lines.extend(
        [
            "",
            "## Phase-time and process medians",
            "",
            "| Workload | State | Lookup | Preflight | Materialization | Publication | rustc worker | Build scripts | Build-script worker |",
            "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
        ]
    )
    for group in summary["groups"]:

        def median(section: str, field: str) -> int | float | None:
            return measurement_median(group, section, field)

        lines.append(
            "| {workload} | {state} | {lookup} | {preflight} | "
            "{materialization} | {publication} | {rustc} | {scripts} | "
            "{script_time} |".format(
                workload=group["workload"],
                state=group["state"],
                lookup=format_milliseconds(median("lookup", "elapsed_us")),
                preflight=format_milliseconds(median("preflight", "elapsed_us")),
                materialization=format_milliseconds(
                    median("materialization", "elapsed_us")
                ),
                publication=format_milliseconds(median("publication", "elapsed_us")),
                rustc=format_milliseconds(median("rustc", "elapsed_us")),
                scripts=format_count(median("build_script", "executions")),
                script_time=format_milliseconds(median("build_script", "elapsed_us")),
            )
        )

    reason_rows = []
    for group in summary["groups"]:
        for reason, values in group["ineligible_by_reason"].items():
            if not values["total"]:
                continue
            reason_rows.append(
                "| {workload} | {state} | `{reason}` | {median} | {total} |".format(
                    workload=group["workload"],
                    state=group["state"],
                    reason=reason,
                    median=format_count(values["median"]),
                    total=format_count(values["total"]),
                )
            )
    lines.extend(
        [
            "",
            "## Ineligible actions by reason",
            "",
            "| Workload | State | Reason | Median per run | Total across trials |",
            "| --- | --- | --- | ---: | ---: |",
            *(reason_rows or ["| n/a | n/a | No reported reasons | n/a | n/a |"]),
            "",
            "Eligibility is eligible / total actions. Effectiveness is hits / eligible actions. ",
            "Total coverage is hits / total actions. `n/a` means the Cargo statistics record did ",
            "not expose that counter; it is not treated as zero. Phase times are overlapping ",
            "cumulative worker time and must not be added together.",
            "",
        ]
    )
    return "\n".join(lines)


def run_benchmark(args: argparse.Namespace) -> None:
    script_root = Path(__file__).resolve().parent.parent
    workspace = Path(args.workspace).expanduser().resolve()
    if not workspace.is_dir():
        raise BenchmarkError(f"workspace does not exist: {workspace}")
    srs = check_repository(
        script_root, args.expect_srs_rev, allow_dirty=args.allow_dirty
    )
    subject = check_repository(
        workspace, args.expect_workspace_rev, allow_dirty=args.allow_dirty
    )
    subject_status = repository_status(workspace)
    cargo = resolve_program(args.cargo)
    config_path = Path(args.config).expanduser().resolve()
    provenance_path = Path(args.toolchain_provenance).expanduser().resolve()
    provenance = load_toolchain_provenance(provenance_path, srs["revision"])
    workloads = load_workloads(config_path)
    selected = [name.strip() for name in args.workloads.split(",") if name.strip()]
    unknown = [name for name in selected if name not in workloads]
    if unknown:
        raise BenchmarkError(f"unknown workloads: {', '.join(unknown)}")
    if not selected:
        raise BenchmarkError("at least one workload must be selected")
    validate_backend_workloads(args.backend, selected)
    cargo_metadata = program_metadata(cargo, ["-Vv"])
    cargo_real = cargo.with_name("cargo-srs-real")
    if not cargo_real.is_file():
        raise BenchmarkError(
            f"installed SRS Cargo is missing sibling executable: {cargo_real}"
        )
    cargo_metadata["real"] = program_metadata(cargo_real, ["-Vv"])
    rustc = cargo.with_name("rustc")
    if not rustc.is_file():
        raise BenchmarkError(
            f"installed SRS Cargo is missing sibling executable: {rustc}"
        )
    rustc_metadata = program_metadata(rustc, ["-Vv"])
    validate_toolchain_executables(provenance, cargo_metadata, rustc_metadata)
    linker_metadata = (
        program_metadata(args.linker, ["--version"]) if args.linker else None
    )
    resolved_linker = linker_metadata["path"] if linker_metadata else None
    workload_tools = {}
    if "test" in selected:
        workload_tools = {
            "cargo-nextest": program_metadata("cargo-nextest", ["--version"]),
            "python3": program_metadata("python3", ["--version"]),
        }
    forbidden = [workspace, script_root, config_path, provenance_path, cargo]
    output = ensure_safe_root(Path(args.output), forbidden)
    target_root = ensure_safe_root(Path(args.target_root), forbidden)
    cache_root = ensure_safe_root(Path(args.cache_root), forbidden)
    validate_independent_roots((output, target_root, cache_root))
    roots = (output, target_root, cache_root)
    for path in roots:
        validate_root_for_prepare(path, overwrite=args.overwrite)
        path.mkdir(parents=True, exist_ok=True)
    if len({path.stat().st_dev for path in roots}) != 1:
        raise BenchmarkError(
            "output, target, and artifact cache roots must be on the same filesystem"
        )
    for path in roots:
        prepare_root(path, overwrite=args.overwrite)
    schedule = balanced_schedule(args.trials, args.seed)
    metadata = {
        "schema_version": SCHEMA_VERSION,
        "created_at": utc_now(),
        "srs": srs,
        "subject": subject,
        "cargo": cargo_metadata,
        "rustc": rustc_metadata,
        "toolchain_provenance": {
            "path": str(provenance_path),
            "sha256": sha256_file(provenance_path),
            "record": provenance,
        },
        "workload_tools": workload_tools,
        "config": {
            "path": str(config_path),
            "sha256": hashlib.sha256(config_path.read_bytes()).hexdigest(),
        },
        "workloads": selected,
        "backend": args.backend,
        "linker": linker_metadata,
        "jobs": args.jobs,
        "materialization": args.materialization,
        "trials": args.trials,
        "seed": args.seed,
        "schedule": schedule,
        "paths": {
            "output": str(output),
            "target_root": str(target_root),
            "cache_root": str(cache_root),
        },
        "host": host_metadata(),
        "filesystems": {
            "workspace": filesystem_metadata(workspace),
            "output": filesystem_metadata(output),
            "target": filesystem_metadata(target_root),
            "cache": filesystem_metadata(cache_root),
        },
    }
    write_json(output / "metadata.json", metadata)
    manifest = output / "runs.jsonl"
    for workload_name in selected:
        workload = workloads[workload_name]
        workload_target = target_root / workload_name
        producer_target = workload_target / "producer"
        consumer_target = workload_target / "consumer"
        workload_cache = cache_root / workload_name
        seed_cache = workload_cache / "seed"
        active_cache = workload_cache / "active"
        remove_tree(producer_target)
        remove_tree(consumer_target)
        remove_tree(seed_cache)
        remove_tree(active_cache)
        seed_cache.mkdir(parents=True)
        population_directory = output / "population" / workload_name
        environment = benchmark_environment(
            os.environ,
            workload.get("environment", {}),
            state="cold",
            active_cache=seed_cache,
            target=producer_target,
            workspace=workspace,
            backend=args.backend,
            linker=resolved_linker,
            jobs=args.jobs,
            materialization=args.materialization,
            run_directory=population_directory,
            toolchain_bin=cargo.parent,
        )
        result = run_invocation(
            cargo=cargo,
            arguments=workload["command"],
            workspace=workspace,
            environment=environment,
            run_directory=population_directory,
            workload=workload_name,
            state="cold",
            trial=0,
            order=0,
            target=producer_target,
            active_cache=seed_cache,
            population=True,
        )
        verify_repository_status(workspace, subject_status)
        result["relative_directory"] = str(population_directory.relative_to(output))
        append_json_line(manifest, result)
        seed_manifest = tree_digest(seed_cache)
        write_json(population_directory / "seed-manifest.json", seed_manifest)
        for trial, states in enumerate(schedule, 1):
            for order, state in enumerate(states, 1):
                remove_tree(consumer_target)
                if state == "warm":
                    reset_active_cache(seed_cache, active_cache)
                else:
                    remove_tree(active_cache)
                    active_cache.mkdir(parents=True)
                run_directory = (
                    output
                    / "runs"
                    / workload_name
                    / f"trial-{trial:02d}"
                    / f"{order:02d}-{state}"
                )
                environment = benchmark_environment(
                    os.environ,
                    workload.get("environment", {}),
                    state=state,
                    active_cache=active_cache,
                    target=consumer_target,
                    workspace=workspace,
                    backend=args.backend,
                    linker=resolved_linker,
                    jobs=args.jobs,
                    materialization=args.materialization,
                    run_directory=run_directory,
                    toolchain_bin=cargo.parent,
                )
                result = run_invocation(
                    cargo=cargo,
                    arguments=workload["command"],
                    workspace=workspace,
                    environment=environment,
                    run_directory=run_directory,
                    workload=workload_name,
                    state=state,
                    trial=trial,
                    order=order,
                    target=consumer_target,
                    active_cache=active_cache,
                )
                verify_repository_status(workspace, subject_status)
                result["relative_directory"] = str(run_directory.relative_to(output))
                append_json_line(manifest, result)
        final_seed_manifest = tree_digest(seed_cache)
        if final_seed_manifest != seed_manifest:
            raise BenchmarkError(f"warm seed changed during {workload_name} trials")
    summary = summarize_records(output)
    write_json(output / "summary.json", summary)
    markdown = markdown_summary(summary)
    (output / "summary.md").write_text(markdown)
    print(markdown, end="")


def summarize_command(args: argparse.Namespace) -> None:
    output = Path(args.output).expanduser().resolve()
    summary = summarize_records(output)
    write_json(output / "summary.json", summary)
    markdown = markdown_summary(summary)
    (output / "summary.md").write_text(markdown)
    if args.format == "json":
        print(json.dumps(summary, indent=2, sort_keys=True))
    else:
        print(markdown, end="")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="subcommand", required=True)
    run = subparsers.add_parser("run", help="run the controlled benchmark matrix")
    run.add_argument("--workspace", required=True, help="pinned subject workspace")
    run.add_argument("--expect-workspace-rev", required=True)
    run.add_argument("--expect-srs-rev", required=True)
    run.add_argument("--cargo", required=True, help="SRS Cargo wrapper executable")
    run.add_argument(
        "--toolchain-provenance",
        required=True,
        help="JSON record binding the installed toolchain artifact to the SRS revision",
    )
    run.add_argument(
        "--config",
        default=str(
            Path(__file__).with_name("artifact-cache-benchmark-workloads.json")
        ),
    )
    run.add_argument("--workloads", default="build,clippy,test")
    run.add_argument("--backend", choices=("llvm", "cranelift"), default="llvm")
    run.add_argument("--linker")
    run.add_argument("--jobs", type=int, default=os.cpu_count() or 1)
    run.add_argument(
        "--materialization", choices=("hardlink", "copy"), default="hardlink"
    )
    run.add_argument("--trials", type=int, default=7)
    run.add_argument("--seed", type=int, default=19754)
    run.add_argument("--target-root", required=True)
    run.add_argument("--cache-root", required=True)
    run.add_argument("--output", required=True)
    run.add_argument("--overwrite", action="store_true")
    run.add_argument("--allow-dirty", action="store_true")
    run.set_defaults(function=run_benchmark)
    summarize = subparsers.add_parser(
        "summarize", help="regenerate a benchmark summary"
    )
    summarize.add_argument("output")
    summarize.add_argument("--format", choices=("markdown", "json"), default="markdown")
    summarize.set_defaults(function=summarize_command)
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    if getattr(args, "jobs", 1) < 1:
        parser.error("--jobs must be at least one")
    if getattr(args, "trials", 1) < 1:
        parser.error("--trials must be at least one")
    try:
        args.function(args)
    except BenchmarkError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
