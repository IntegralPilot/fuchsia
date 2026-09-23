#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A script used to compare the outputs of two different GN binaries.

This tool is useful to compare the full `gn gen` output of two
different GN binaries for all Fuchsia trybot configurations.

For maximum efficiency and correctness:

- Use this in a checkout that includes //vendor/google-smart
  as well as //vendor/google (if comparing internal configs)

- Use this on a system that supports `cp --reflink=always`
  such as btrfs.

It will do the following:

- Scan integration/infra/config/generated/{fuchsia,turquoise}/fint_params/try
  to list all the fint params files used by Fuchsia CQ.

- Bootstrap fint from sources (to ensure it's using the one
  corresponding to the current checkout).

- Find all fint params for configurations that match the current
  checkout (e.g. if the configuration requires //vendor/google-smart
  and it's not in the checkout, it will be skipped automatically).

- Create an out/gn_compare/ directory where all outputs will be
  stored.

For each build configuration, then:

- Use `fint set` with the right parameters from its fint_params
  file, to setup `out/compare_gn.run/args.gn`.

- Save the clean state of `out/compare_gn.run` to `out/compare_gn.template`.

- Run `gn1 gen out/compare_gn.run`, then
  move the result to out/gn_compare/<config_name>.1.

- Restore the clean state from `out/compare_gn.template` to `out/compare_gn.run`.

- Run `gn2 gen out/compare_gn.run` then
  move the result to out/gn_compare/<config_name>.2.

- Report which files are different between <config_name>.1
  and <config_name>.2 to let the user inspect the differences
  manually.

Use the --config-count=NUMBER to limit the number of configurations
to compare. This is useful during debugging.

Use the --config-filter=PATTERN to limit the comparison to configurations
matching a specific regex pattern (substring by default).
"""

import argparse
import filecmp
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def find_fint_params(fuchsia_dir: Path) -> list[Path]:
    """Finds all fint_params files in both fuchsia and turquoise try directories."""
    configs: list[Path] = []
    for project in ["fuchsia", "turquoise"]:
        params_dir = (
            fuchsia_dir
            / f"integration/infra/config/generated/{project}/fint_params/try"
        )
        if params_dir.exists():
            configs.extend(params_dir.glob("*.textproto"))
    return configs


def check_config_valid(fuchsia_dir: Path, params_path: Path) -> bool:
    """Checks if the config references any non-existent vendor directories."""
    with open(params_path, "r") as f:
        content = f.read()

    # Find all //vendor/... strings
    vendor_paths = re.findall(r'"(//vendor/[^"]+)"', content)
    for vp in vendor_paths:
        # Remove // and split at :
        rel_path = vp.removeprefix("//").split(":")[0]
        abs_path = fuchsia_dir / rel_path
        if not abs_path.exists():
            return False
    return True


def bootstrap_fint(fuchsia_dir: Path, tmp_dir: Path) -> Path:
    """Bootstraps fint tool."""
    bootstrap_script = fuchsia_dir / "tools/integration/bootstrap.sh"
    fint_path = tmp_dir / "fint"
    print("Bootstrapping fint...")
    subprocess.check_call(
        [str(bootstrap_script), "-o", str(fint_path)], cwd=fuchsia_dir
    )
    return fint_path


def generate_context_proto(
    tmp_dir: Path, fuchsia_dir: Path, build_dir: Path
) -> Path:
    """Generates a context.textproto file."""
    context_path = tmp_dir / "context.textproto"
    artifact_dir = build_dir / "artifacts"
    artifact_dir.mkdir(parents=True, exist_ok=True)

    content = f"""checkout_dir: "{fuchsia_dir}"
build_dir: "{build_dir}"
artifact_dir: "{artifact_dir}"
"""
    context_path.write_text(content)
    return context_path


def run_fint_set(
    fint_path: Path, static_proto: Path, context_proto: Path, fuchsia_dir: Path
) -> bool:
    """Runs fint set to initialize the build directory."""
    cmd = [
        str(fint_path),
        "set",
        "-static",
        str(static_proto),
        "-context",
        str(context_proto),
    ]
    try:
        subprocess.run(
            cmd, check=True, capture_output=True, text=True, cwd=fuchsia_dir
        )
        return True
    except subprocess.CalledProcessError as e:
        print(f"fint set failed for {static_proto.name}:", file=sys.stderr)
        print(e.stderr, file=sys.stderr)
        return False


def copy_dir_reflink(src: Path, dst: Path) -> bool:
    """Copies a directory using btrfs reflink if possible. Cleans up dst on failure."""
    try:
        dst.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            ["cp", "--reflink=always", "-rp", str(src), str(dst)],
            check=True,
            capture_output=True,
        )
        return True
    except subprocess.CalledProcessError:
        if dst.exists():
            shutil.rmtree(dst)
        return False


def move_dir(src: Path, dst: Path) -> bool:
    """Moves a directory, trying rename first, then reflink copy, then regular copy."""
    try:
        os.rename(src, dst)
        return True
    except OSError:
        # Ensure destination is clean before fallback copy
        if dst.exists():
            shutil.rmtree(dst)
        if not copy_dir_reflink(src, dst):
            shutil.copytree(src, dst)
        shutil.rmtree(src)
        return False


def compare_directories(dir1: Path, dir2: Path) -> list[str]:
    """Recursively compares two directories, returning list of differences."""
    diffs = []

    files1 = set(p.relative_to(dir1) for p in dir1.rglob("*") if p.is_file())
    files2 = set(p.relative_to(dir2) for p in dir2.rglob("*") if p.is_file())

    only_in_1 = files1 - files2
    only_in_2 = files2 - files1

    for f in only_in_1:
        diffs.append(f"Only in GN1: {f}")
    for f in only_in_2:
        diffs.append(f"Only in GN2: {f}")

    common = files1 & files2
    for f in common:
        f1 = dir1 / f
        f2 = dir2 / f

        if f.name in [
            "args.gn",
            "set_artifacts.json",
            "gn_trace.json",
            "command-line.txt",
        ]:
            continue
        if "artifacts" in f.parts:
            continue

        if not filecmp.cmp(f1, f2, shallow=False):
            diffs.append(f"Content differs: {f}")

    return diffs


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--gn1",
        required=True,
        type=Path,
        help="Path to the first GN binary (current).",
    )
    parser.add_argument(
        "--gn2",
        required=True,
        type=Path,
        help="Path to the second GN binary (modified).",
    )
    parser.add_argument(
        "--fuchsia-dir",
        type=Path,
        help="Fuchsia directory.",
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        help="Output directory for comparison copies (defaults to <fuchsia-dir>/out/compare_gn).",
    )
    parser.add_argument(
        "--config-count",
        type=int,
        help="Limit the comparison to a fixed number of configurations to save time.",
    )
    parser.add_argument(
        "--config-filter",
        help="Regex filter for configuration names (e.g. 'core.x64').",
    )

    args = parser.parse_args()

    if not args.fuchsia_dir:
        fuchsia_dir_env = os.getenv("FUCHSIA_DIR")
        if not fuchsia_dir_env:
            print(
                "Error: --fuchsia-dir or FUCHSIA_DIR env var is required.",
                file=sys.stderr,
            )
            return 1
        fuchsia_dir = Path(fuchsia_dir_env)
    else:
        fuchsia_dir = args.fuchsia_dir

    fuchsia_dir = fuchsia_dir.resolve()
    gn1 = args.gn1.resolve()
    gn2 = args.gn2.resolve()

    if not gn1.exists():
        print(f"Error: GN1 binary not found at {gn1}", file=sys.stderr)
        return 1
    if not gn2.exists():
        print(f"Error: GN2 binary not found at {gn2}", file=sys.stderr)
        return 1

    out_dir = args.out_dir or fuchsia_dir / "out/compare_gn"
    out_dir = out_dir.resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    print(f"Fuchsia Dir: {fuchsia_dir}")
    print(f"GN1:         {gn1}")
    print(f"GN2:         {gn2}")
    print(f"Out Dir:     {out_dir}")

    configs = find_fint_params(fuchsia_dir)
    print(f"Found {len(configs)} configurations.")

    valid_configs = [c for c in configs if check_config_valid(fuchsia_dir, c)]
    print(
        f"Valid configurations (matching current checkout): {len(valid_configs)}"
    )

    if args.config_filter:
        filter_re = re.compile(args.config_filter)
        valid_configs = [c for c in valid_configs if filter_re.search(c.stem)]
        print(
            f"Filtered to {len(valid_configs)} configurations matching"
            f" '{args.config_filter}'."
        )

    if args.config_count is not None:
        valid_configs = valid_configs[: args.config_count]
        print(
            f"Limiting comparison to the first {args.config_count} valid"
            " configurations."
        )

    if not valid_configs:
        print("No valid configurations to test.")
        return 0

    with tempfile.TemporaryDirectory() as tmp_dir1:
        tmp_dir = Path(tmp_dir1)
        fint_path = bootstrap_fint(fuchsia_dir, tmp_dir)

        # The report is a list of (config_name, message, error_list) tuples
        report: list[tuple[str, str, list[str]]] = []

        for config in valid_configs:
            config_name = config.stem
            print(f"\n--- Processing {config_name} ---")

            # Run directory MUST be directly under out/ to avoid nesting issues
            run_dir = out_dir.parent / "compare_gn.run"
            run_dir_template = out_dir.parent / "compare_gn.template"
            gn1_dir = out_dir / f"{config_name}.1"
            gn2_dir = out_dir / f"{config_name}.2"

            # Cleanup previous runs
            for d in [run_dir, run_dir_template, gn1_dir, gn2_dir]:
                if d.exists():
                    shutil.rmtree(d)

            try:
                # 1. Initialize with fint set in run_dir
                context_proto = generate_context_proto(
                    tmp_dir, fuchsia_dir, run_dir
                )
                print("Initializing build directory...")
                if not run_fint_set(
                    fint_path, config, context_proto, fuchsia_dir
                ):
                    print(
                        f"Skipping {config_name} due to initialization failure."
                    )
                    report.append((config_name, "INIT_FAILED", []))
                    continue

                # Save clean state to template
                if not copy_dir_reflink(run_dir, run_dir_template):
                    shutil.copytree(run_dir, run_dir_template)

                # 2. Run GN1 in run_dir
                print("Running GN1...")
                try:
                    subprocess.run(
                        [str(gn1), "gen", str(run_dir)],
                        check=True,
                        capture_output=True,
                        text=True,
                    )
                except subprocess.CalledProcessError as e:
                    print(f"GN1 failed: {e.stderr}")
                    report.append((config_name, "GN1_FAILED", [e.stderr]))
                    continue

                # 3. Move GN1 output to gn1_dir
                print("Saving GN1 output...")
                move_dir(run_dir, gn1_dir)

                # 4. Restore clean state for GN2
                print("Restoring clean build directory...")
                move_dir(run_dir_template, run_dir)

                # 5. Run GN2 in run_dir
                print("Running GN2...")
                try:
                    subprocess.run(
                        [str(gn2), "gen", str(run_dir)],
                        check=True,
                        capture_output=True,
                        text=True,
                    )
                except subprocess.CalledProcessError as e:
                    print(f"GN2 failed: {e.stderr}")
                    report.append((config_name, "GN2_FAILED", [e.stderr]))
                    continue

                # 6. Move GN2 output to gn2_dir
                print("Saving GN2 output...")
                move_dir(run_dir, gn2_dir)

                # 7. Compare
                print("Comparing outputs...")
                diffs = compare_directories(gn1_dir, gn2_dir)
                if diffs:
                    print(f"Found {len(diffs)} discrepancies!")
                    for d in diffs:
                        print(f"  {d}")
                    report.append((config_name, "DIFFERENT", diffs))
                else:
                    print("Outputs are identical.")
                    report.append((config_name, "IDENTICAL", []))

            finally:
                # Clean up any remaining temp directories for this config (on failure/continue)
                for d in [run_dir, run_dir_template]:
                    if d.exists():
                        shutil.rmtree(d)

        # Generate Report
        print("\n=========================================")
        print("          GN COMPARISON REPORT           ")
        print("=========================================")

        identical_count = 0
        different_count = 0
        failed_count = 0

        for name, status, diffs in report:
            if status == "IDENTICAL":
                identical_count += 1
            elif status == "DIFFERENT":
                different_count += 1
                print(f"\nConfig: {name} (DIFFERENT)")
                for d in diffs:
                    print(f"  {d}")
            else:
                failed_count += 1
                print(f"\nConfig: {name} (FAILED: {status})")
                if diffs:
                    print(f"  Error: {diffs[0]}")

        print("\nSummary:")
        print(f"  Identical: {identical_count}")
        print(f"  Different: {different_count}")
        print(f"  Failed:    {failed_count}")
        return 0


if __name__ == "__main__":
    sys.exit(main())
