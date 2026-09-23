# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import asyncio
import dataclasses
import os
import tempfile

import async_utils.command as command
from fx_cmd.lib import FxCmd
import statusinfo

import args
import environment
import event
import execution


async def get_diff_base(recorder: event.EventRecorder | None = None) -> str:
    """Determines the base commit for diffing, aligning with fx format-code."""
    upstream_res = await execution.run_command(
        "git",
        "rev-parse",
        "--abbrev-ref",
        "--symbolic-full-name",
        "@{u}",
        recorder=recorder,
    )
    upstream = ""
    if upstream_res and upstream_res.return_code == 0 and upstream_res.stdout:
        upstream = upstream_res.stdout.strip()

    if not upstream:
        ancestor_res = await execution.run_command(
            "git",
            "merge-base",
            "--is-ancestor",
            "origin/main",
            "HEAD",
            recorder=recorder,
        )
        if ancestor_res and ancestor_res.return_code == 0:
            upstream = "origin/main"
        else:
            ancestor_res = await execution.run_command(
                "git",
                "merge-base",
                "--is-ancestor",
                "JIRI_HEAD",
                "HEAD",
                recorder=recorder,
            )
            if ancestor_res and ancestor_res.return_code == 0:
                ancestor_res2 = await execution.run_command(
                    "git",
                    "merge-base",
                    "--is-ancestor",
                    "JIRI_HEAD",
                    "origin/main",
                    recorder=recorder,
                )
                if ancestor_res2 and ancestor_res2.return_code == 0:
                    mb_res = await execution.run_command(
                        "git",
                        "merge-base",
                        "HEAD",
                        "origin/main",
                        recorder=recorder,
                    )
                    if mb_res and mb_res.return_code == 0 and mb_res.stdout:
                        upstream = mb_res.stdout.strip()
                else:
                    upstream = "JIRI_HEAD"
            else:
                upstream = "HEAD"

    if upstream == "HEAD":
        return "HEAD"

    rev_list_res = await execution.run_command(
        "git",
        "rev-list",
        "HEAD",
        f"^{upstream}",
        "--",
        recorder=recorder,
    )

    if (
        not rev_list_res
        or rev_list_res.return_code != 0
        or not rev_list_res.stdout
    ):
        return "HEAD"

    lines = rev_list_res.stdout.splitlines()
    if not lines:
        return "HEAD"

    local_commit = lines[-1].strip()

    parent_res = await execution.run_command(
        "git",
        "rev-parse",
        f"{local_commit}^",
        recorder=recorder,
    )

    if parent_res and parent_res.return_code == 0 and parent_res.stdout:
        return parent_res.stdout.strip()

    return "HEAD"


async def get_dirty_files(
    fuchsia_dir: str,
    affected_since: str | None = None,
    recorder: event.EventRecorder | None = None,
) -> list[str] | None:
    """Invokes git to identify modified files.

    Args:
        fuchsia_dir: The Fuchsia directory.
        affected_since: If set, compare against this commit/branch instead of checking uncommitted files.
        recorder: The event recorder.

    Returns:
        A list of modified files relative to fuchsia_dir, or None if clean/invalid.
    """
    toplevel_res = await execution.run_command(
        "git",
        "rev-parse",
        "--show-toplevel",
        recorder=recorder,
    )
    if not toplevel_res or toplevel_res.return_code != 0:
        if recorder:
            stdout = toplevel_res.stdout if toplevel_res else "None"
            stderr = toplevel_res.stderr if toplevel_res else "None"
            recorder.emit_instruction_message(
                "ERROR: You must run fx test from inside a git repository to use --show-affected-tests.\n"
                + f"STDOUT: {stdout}\n"
                + f"STDERR: {stderr}"
            )
        return None

    repo_root = toplevel_res.stdout.strip() if toplevel_res.stdout else ""
    if recorder:
        recorder.emit_instruction_message(f"Querying repository: {repo_root}")

    dirty_files: list[str] = []
    diff_base = affected_since
    if not diff_base:
        diff_base = await get_diff_base(recorder)

    if recorder:
        recorder.emit_instruction_message(
            f"Querying files changed since {diff_base}"
        )

    diff_args = ["git", "diff", "--name-only", diff_base]

    diff_res = await execution.run_command(
        *diff_args,
        recorder=recorder,
    )

    if not diff_res or diff_res.return_code != 0:
        if recorder:
            stdout = diff_res.stdout if diff_res else "None"
            stderr = diff_res.stderr if diff_res else "None"
            recorder.emit_warning_message(
                f"Failed to run git diff against {diff_base}\n"
                + f"STDOUT: {stdout}\n"
                + f"STDERR: {stderr}"
            )
        return None

    for line in diff_res.stdout.splitlines():
        rel_path = line.strip()
        if not rel_path:
            continue
        abs_path = os.path.join(repo_root, rel_path)
        dirty_files.append(os.path.relpath(abs_path, fuchsia_dir))

    if not dirty_files:
        msg = (
            f"\nNo files changed since {affected_since}."
            if affected_since
            else "\nYour repository is completely clean. No files are modified, so no tests are affected."
        )
        if recorder:
            recorder.emit_instruction_message(msg)
        return None

    return dirty_files


@dataclasses.dataclass(frozen=True)
class BuildConfig:
    """Represents a product/board configuration for finding affected tests."""

    product_board: str
    with_args: list[str]


@dataclasses.dataclass(frozen=True)
class AffectedResult:
    """Represents a single affected test target result from the build API."""

    label: str
    is_host: bool


@dataclasses.dataclass(frozen=True)
class GatheredResult:
    """Represents the set of affected tests for a specific build configuration."""

    product_board: str
    affected_results: list[AffectedResult]


@dataclasses.dataclass(frozen=True)
class FormattedResult:
    """Represents the aggregated configuration info for a specific test label."""

    is_host: bool
    pb_configs: list[str]


@dataclasses.dataclass(frozen=True)
class AffectedTarget:
    """Represents a matched affected test target formatted for display."""

    pure_label: str
    pb_configs: list[str]
    command: str
    is_host: bool = False


def format_affected_targets(
    label_to_results: dict[str, FormattedResult]
) -> list[AffectedTarget]:
    """Sorts and formats affected targets into their respective commands."""
    results: list[AffectedTarget] = []
    for label, res in sorted(label_to_results.items()):
        pure_label = label.split("(")[0]
        if res.is_host:
            cmd = f"fx add-host-test {pure_label}"
        else:
            cmd = f"fx add-test {pure_label}"
        results.append(
            AffectedTarget(pure_label, res.pb_configs, cmd, is_host=res.is_host)
        )
    return results


def format_affected_commands(targets: list[AffectedTarget]) -> list[str]:
    """Formats grouped command lines for all identified affected targets."""
    commands: list[str] = []
    device_targets = [t.pure_label for t in targets if not t.is_host]
    host_targets = [t.pure_label for t in targets if t.is_host]
    if device_targets:
        commands.append(f"fx add-test {' '.join(device_targets)}")
    if host_targets:
        commands.append(f"fx add-host-test {' '.join(host_targets)}")
    return commands


def clean_gathered_results(
    results: list[GatheredResult],
) -> dict[str, FormattedResult]:
    """Maps explicitly affected test GN labels back to configured board products."""
    label_to_results: dict[str, FormattedResult] = {}
    for res in results:
        pb = res.product_board
        for affected in res.affected_results:
            label = affected.label
            is_host = affected.is_host
            if not label or not label.startswith(("//", "@")):
                continue
            if label not in label_to_results:
                label_to_results[label] = FormattedResult(is_host, [])
            # Append configs
            if pb not in label_to_results[label].pb_configs:
                label_to_results[label].pb_configs.append(pb)
    return label_to_results


def parse_build_api_output(
    out_client: command.CommandOutput | None,
) -> list[AffectedResult]:
    """Parses the output from build/api/client affected_tests.

    Returns:
        A list of AffectedResult objects.
    """
    if not out_client or out_client.return_code != 0:
        return []

    out_text = out_client.stdout.strip() if out_client.stdout else ""
    parsed_results: list[AffectedResult] = []
    for line in out_text.splitlines():
        line = line.strip()
        if not line:
            continue
        parts = line.split(",")
        if len(parts) >= 2:
            parsed_results.append(AffectedResult(parts[0], parts[1] == "host"))
        else:
            parsed_results.append(AffectedResult(parts[0], False))
    return parsed_results


async def find_affected_tests(
    fuchsia_dir: str,
    product_board: str,
    out_dir: str,
    with_args: list[str],
    files_list: str,
) -> GatheredResult:
    """Configures a temporary build graph and finds affected tests using build-api-client.

    Returns:
        A GatheredResult object containing board info and affected test list.
    """
    fx = FxCmd(build_directory=out_dir)
    set_args = [
        "set",
        product_board,
        "--no-change-env",
        "--rbe-mode=off",
    ]
    for w in with_args:
        set_args.extend(["--with", w])

    try:
        running_fx = await fx.start(*set_args)
        out_set = await running_fx.run_to_completion()
        if out_set.return_code != 0:
            return GatheredResult(product_board, [])

        client_cmd = [
            f"{fuchsia_dir}/build/api/client",
            "--build-dir",
            out_dir,
            "affected_tests",
            f"--files-list={files_list}",
        ]

        out_client = await execution.run_command(
            *client_cmd,
        )

        return GatheredResult(product_board, parse_build_api_output(out_client))

    except Exception:
        return GatheredResult(product_board, [])


async def get_affected_targets(
    exec_env: environment.ExecutionEnvironment,
    affected_since: str | None = None,
    recorder: event.EventRecorder | None = None,
) -> list[AffectedTarget]:
    """Orchestrates discovering which tests are affected by dirty files across several different configurations."""

    dirty_files = await get_dirty_files(
        exec_env.fuchsia_dir, affected_since, recorder
    )
    if dirty_files is None:
        return []

    if recorder:
        recorder.emit_instruction_message(
            f"Found {len(dirty_files)} modified file(s)."
        )

    with tempfile.NamedTemporaryFile(
        mode="w", suffix=".txt", delete=True
    ) as dirty_files_file:
        dirty_files_file.write("\n".join(dirty_files) + "\n")
        # Ensure it's fully flushed to disk!
        dirty_files_file.flush()
        dirty_files_path = dirty_files_file.name

        with tempfile.TemporaryDirectory(dir=exec_env.out_dir) as out_tmp_dir:
            build_configurations = [
                BuildConfig("core.x64", ["//bundles/buildbot/core"]),
                BuildConfig("core.arm64", ["//bundles/buildbot/core"]),
                BuildConfig("minimal.x64", ["//bundles/buildbot/minimal"]),
                BuildConfig("minimal.arm64", ["//bundles/buildbot/minimal"]),
                BuildConfig(
                    "bringup_with_tests.x64", ["//bundles/buildbot/bringup"]
                ),
                BuildConfig(
                    "bringup_with_tests.arm64", ["//bundles/buildbot/bringup"]
                ),
            ]

            group_id = None
            if recorder:
                group_id = recorder.emit_event_group(
                    "Finding affected tests",
                    queued_events=len(build_configurations),
                )

            async def run_find_affected(
                config: BuildConfig,
            ) -> GatheredResult:
                child_id = None
                if recorder:
                    child_id = recorder.emit_event_group(
                        f"Checking {config.product_board}", parent=group_id
                    )
                try:
                    pb_slug = config.product_board.replace(".", "_")
                    out_dir = os.path.join(out_tmp_dir, pb_slug)

                    return await find_affected_tests(
                        exec_env.fuchsia_dir,
                        config.product_board,
                        out_dir,
                        config.with_args,
                        dirty_files_path,
                    )
                finally:
                    if recorder and child_id:
                        recorder.emit_end(id=child_id)

            if recorder:
                recorder.emit_instruction_message(
                    f"Spawning parallel evaluations for {len(build_configurations)} product/board configurations (this might take a minute)..."
                )

            try:
                results = await asyncio.gather(
                    *(
                        run_find_affected(config)
                        for config in build_configurations
                    )
                )
            finally:
                if recorder and group_id:
                    recorder.emit_end(id=group_id)

            label_to_results = clean_gathered_results(results)
            if not label_to_results:
                return []

            return format_affected_targets(label_to_results)


async def show_affected_tests(
    exec_env: environment.ExecutionEnvironment,
    flags: args.Flags,
    recorder: event.EventRecorder,
) -> None:
    """Orchestrates discovering which tests are affected by dirty files across several different configurations."""
    targets = await get_affected_targets(
        exec_env, flags.affected_since, recorder
    )

    if not targets:
        recorder.emit_info_message(
            "\nNone of your modified files affect any known tests across the 6 major configurations."
        )
        return

    recorder.emit_info_message(
        f"\nFound {len(targets)} affected test(s) matching your uncommitted files:\n"
    )

    for target in targets:
        recorder.emit_verbatim_message(
            f"{statusinfo.highlight(target.pure_label, style=flags.style)} ({', '.join(target.pb_configs)})"
        )

    for cmd in format_affected_commands(targets):
        dim_cmd = statusinfo.dim(f"\n  > {cmd}", style=flags.style)
        recorder.emit_verbatim_message(dim_cmd)
