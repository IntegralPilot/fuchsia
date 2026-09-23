#!/usr/bin/env fuchsia-vendored-python
#
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Runs clippy on a set of gn targets or rust source files

import argparse
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

from rust import (
    FUCHSIA_BUILD_DIR,
    ROOT_PATH,
    GnTarget,
)


def possible_labels(t: dict[str, any], fuchsia_dir: str) -> set[str]:
    """all labels the user could have meant"""
    labels = set()
    for key in ("clippy_label", "original_label", "actual_label"):
        if key in t and t[key]:
            gn_t = GnTarget(t[key], fuchsia_dir)
            labels.add(str(gn_t))
            # For Fuchsia target labels (such as Zircon kernel modules), GN
            # attaches variant toolchain qualifiers (e.g.
            # `(//zircon/kernel/switch/set/lk_debug_level_2:...)`).
            # Adding the toolchain-stripped label allows user input like `fx
            # clippy //zircon/kernel/bin:vmzircon.with-tests` or file-based
            # target lookups to match without requiring explicit toolchain
            # suffixes.
            if t.get("target_is_fuchsia"):
                labels.add(f"//{gn_t.label_path}:{gn_t.label_name}")
    return labels


def main():
    args = parse_args()

    build_dir = Path(args.out_dir) if args.out_dir else FUCHSIA_BUILD_DIR

    rust_target_mapping = Path(
        build_dir, "rust_target_mapping.json"
    ).read_bytes()
    rust_target_mapping = json.loads(rust_target_mapping)
    rust_target_mapping = [
        t for t in rust_target_mapping if not t["disable_clippy"]
    ]
    if not args.all:
        # filter rust target mapping
        if args.files:
            input_files = {os.path.relpath(f, build_dir) for f in args.input}
            rust_target_mapping = [
                t
                for t in rust_target_mapping
                for s in t["src"]
                if s in input_files
            ]
        else:  # they are labels
            targets_look_like_sources = [
                t for t in args.input if t.endswith(".rs")
            ]
            if targets_look_like_sources:
                print(
                    f"Warning: {targets_look_like_sources} looks like a source file rather than a target, "
                    "maybe you meant to use --files ?"
                )
            rust_target_mapping = [
                t
                for t in rust_target_mapping
                for l in args.input
                if (
                    any(
                        pl.startswith(l[:-2])
                        for pl in possible_labels(t, args.fuchsia_dir)
                    )
                    if l.endswith("**")
                    else str(GnTarget(l, args.fuchsia_dir))
                    in possible_labels(t, args.fuchsia_dir)
                )
            ]

    clippy_outputs = list(
        set(str(Path(t["clippy_output"])) for t in rust_target_mapping)
    )

    if args.get_outputs:
        print(*clippy_outputs, sep="\n")
        return 0

    if not clippy_outputs:
        print("Error: Couldn't find any clippy outputs for those inputs")
        return 1

    if args.no_build:
        run_time = 0
        returncode = 0
    else:
        run_time = time.time()
        returncode = build_targets(clippy_outputs, build_dir, args).returncode

    lints = {}
    for clippy_output in clippy_outputs:
        clippy_output = build_dir / clippy_output
        # If we failed to build all targets we can keep going and print any
        # lints that were collected.
        if returncode != 0:
            if not clippy_output.exists():
                continue
            if os.path.getmtime(clippy_output) < run_time:
                continue
        with open(clippy_output) as f:
            error_reported = False
            for line in f:
                try:
                    lint = json.loads(line)
                except json.decoder.JSONDecodeError:
                    if not error_reported:
                        print(f"Malformed output: {clippy_output}")
                        returncode = 1
                        error_reported = True
                    continue
                # filter out "n warnings emitted" messages
                if not lint["spans"]:
                    continue
                lints[fingerprint_diagnostic(lint)] = lint

    if not args.raw:
        print("\nClippy Diagnostics\n==================\n")
    for lint in lints.values():
        print(json.dumps(fix_paths(lint)) if args.raw else lint["rendered"])
    if not args.raw:
        print(len(lints), "warning(s) emitted\n")

    return returncode


# To deduplicate lints, use the message, code, all top level spans, and macro
# expansion spans
def fingerprint_diagnostic(lint):
    code = lint.get("code")

    def expand_spans(span):
        yield span
        if expansion := span.get("expansion"):
            yield from expand_spans(expansion["span"])

    spans = [x for span in lint["spans"] for x in expand_spans(span)]

    return (
        lint["message"],
        code.get("code") if code else None,
        frozenset(
            (x["file_name"], x["byte_start"], x["byte_end"]) for x in spans
        ),
    )


_RESOLVED_PATH_CACHE = {}


def resolve_generated_kernel_path(file_path):
    """Resolves generated Zircon kernel module paths back to in-tree source files.

    Zircon kernel Rust code relies on GN kernel_rust_mod() templates to produce
    synthetic wrapper `.rs` files under `gen/`. These generated wrappers
    textually pull in real in-tree source files via `include!()`,
    `include_str!()`, or `include_bytes!()`.

    When Clippy reports diagnostics against these generated `gen/` files, IDE
    integrations (like VSCode flycheck) fail to locate the files in the VFS or
    report incorrect line spans. This function inspects generated `.rs` wrapper
    files for include macro directives to map diagnostic spans back to the
    actual repository source files under `zircon/kernel/`.
    """
    if file_path in _RESOLVED_PATH_CACHE:
        return _RESOLVED_PATH_CACHE[file_path]

    abs_path = (
        os.path.join(FUCHSIA_BUILD_DIR, file_path)
        if not os.path.isabs(file_path)
        else file_path
    )
    norm_path = os.path.normpath(abs_path)

    res = norm_path
    if "/gen/" in norm_path and norm_path.endswith(".rs"):
        dir_path = os.path.dirname(norm_path)
        candidates = [norm_path]
        if os.path.exists(dir_path):
            candidates.extend(
                [
                    os.path.join(dir_path, f)
                    for f in os.listdir(dir_path)
                    if f.endswith(".rs")
                ]
            )
        for cand in candidates:
            if os.path.isfile(cand):
                try:
                    with open(
                        cand, "r", encoding="utf-8", errors="ignore"
                    ) as f:
                        content = f.read()
                    includes = re.findall(
                        r'include(?:_str|_bytes)?!\("([^"]+)"\);', content
                    )
                    for inc in includes:
                        resolved_inc = os.path.normpath(
                            os.path.join(os.path.dirname(cand), inc)
                        )
                        if "/gen/" not in resolved_inc and os.path.isfile(
                            resolved_inc
                        ):
                            res = resolved_inc
                            break
                except Exception:
                    pass
            if res != norm_path:
                break

    rel_res = os.path.relpath(res)
    _RESOLVED_PATH_CACHE[file_path] = rel_res
    return rel_res


# Rewrite paths in a diagnostic to be relative to the current directory.
def fix_paths(lint):
    for span in lint["spans"]:
        span["file_name"] = resolve_generated_kernel_path(span["file_name"])
    lint["children"] = [fix_paths(child) for child in lint["children"]]
    return lint


def build_targets(clippy_outputs, build_dir, args):
    fuchsia_dir = Path(args.fuchsia_dir) if args.fuchsia_dir else ROOT_PATH
    if not fuchsia_dir or not fuchsia_dir.is_dir():
        fuchsia_dir = Path(__file__).resolve().parents[5]

    main_build_script = fuchsia_dir / "build" / "scripts" / "main_build.py"

    cmd = [
        sys.executable,
        str(main_build_script),
        "--build-dir",
        str(build_dir),
    ]
    if args.verbose:
        cmd.append("--verbose")
    if args.quiet or args.raw:
        cmd.append("--no-status")

    cmd.append("ninja")

    if args.stop_on_first_error:
        cmd += ["-k", "1"]
    else:
        cmd += ["-k", "0"]
    if args.jobs is not None:
        cmd += ["-j", str(args.jobs)]

    if args.verbose:
        cmd += ["--verbose"]
    if args.quiet:
        cmd += ["--quiet"]
    cmd += clippy_outputs

    output = sys.stderr if args.raw else None
    env = dict(os.environ)
    env["FUCHSIA_DIR"] = str(fuchsia_dir)
    env.setdefault("NINJA_PERSISTENT_MODE", "1")
    if args.raw:
        env.pop("FX_BUILD_RBE_STATS", None)

    if args.verbose:
        print(" ".join([str(arg) for arg in cmd]))
    return subprocess.run(cmd, stdout=output, env=env)


def parse_args():
    parser = argparse.ArgumentParser(
        description="Run cargo clippy on a set of targets or rust files"
    )
    parser.add_argument(
        "--verbose",
        "-v",
        help="verbose build output",
        action="store_true",
        default=False,
    )
    parser.add_argument(
        "--quiet",
        help="don't show progress status",
        action="store_true",
        default=os.environ.get("FX_BUILD_QUIET") == "1",
    )
    parser.add_argument(
        "--files",
        "-f",
        action="store_true",
        help="treat the inputs as source files rather than gn targets",
    )
    inputs = parser.add_mutually_exclusive_group(required=True)
    inputs.add_argument(
        "input",
        nargs="*",
        help="GN targets to run. Strings ending in '**' match any targets up to the glob",
        default=[],
    )
    inputs.add_argument(
        "--all", action="store_true", help="run on all clippy targets"
    )
    advanced = parser.add_argument_group("advanced")
    advanced.add_argument(
        "--out-dir", help="path to the Fuchsia build directory"
    )
    advanced.add_argument(
        "--fuchsia-dir", help="path to the Fuchsia root directory"
    )
    advanced.add_argument(
        "--raw",
        action="store_true",
        help="emit full json rather than human readable messages",
    )
    advanced.add_argument(
        "--get-outputs",
        action="store_true",
        help="emit a list of clippy output files rather than lints",
    )
    advanced.add_argument(
        "--no-build",
        action="store_true",
        help="don't build the clippy output, instead expect that it already exists",
    )
    advanced.add_argument(
        "--stop-on-first-error",
        action="store_true",
        help="stop on first error",
        default=False,
    )
    advanced.add_argument(
        "--jobs",
        "-j",
        type=int,
        help="number of concurrent jobs to run",
        default=None,
    )
    args = parser.parse_args()
    if args.verbose:
        args.quiet = False
    return args


if __name__ == "__main__":
    sys.exit(main())
