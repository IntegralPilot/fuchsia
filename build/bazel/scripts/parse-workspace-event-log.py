#!/usr/bin/env python3
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Convert the binary Bazel workspace log to text format."""

import argparse
import os
import shlex
import subprocess
import sys

_SCRIPT_DIR = os.path.dirname(__file__)
_FUCHSIA_DIR = os.path.abspath(os.path.join(_SCRIPT_DIR, "..", "..", ".."))

sys.path.insert(0, _SCRIPT_DIR)
import build_utils


def find_default_log_file() -> str | None:
    """Find the location of the default log file.

    Returns:
        Path to the default log file, or None if it could not be determined.
    """
    fuchsia_dir = build_utils.find_fuchsia_dir()
    if not fuchsia_dir:
        return None

    build_dir = build_utils.find_fx_build_dir(fuchsia_dir)
    if not build_dir:
        return None

    top_dir = os.path.join(build_dir, build_utils.get_bazel_topdir(fuchsia_dir))
    log_file = os.path.join(top_dir, "logs", "workspace-events.log")
    return log_file


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--jre",
        help="Specify Java JRE used to run the parser, default uses the prebuilt Bazel JRE.",
    )
    parser.add_argument(
        "--log-parser-jar",
        help="Specify alternative location for parser .jar file.",
    )
    parser.add_argument(
        "--exclude_rule",
        action="append",
        default=[],
        help="Rule(s) to filter out while parsing.",
    )
    parser.add_argument(
        "--output_path",
        help="Output file location. If not used, output goes to stdout.",
    )
    parser.add_argument("--log_file", help="Input log file (auto-detected).")
    parser.add_argument(
        "--verbose", action="store_true", help="Enable verbose mode."
    )
    args = parser.parse_args()

    # Find the Java Runtime Environment to run the parser first.
    java_binary = os.path.join("bin", "java")
    if sys.platform.startswith("win"):
        java_binary += ".exe"

    def find_java_binary(jre_path: str) -> str | None:
        path = os.path.join(jre_path, java_binary)
        return path if os.path.exists(path) else None

    if args.jre:
        java_launcher = find_java_binary(args.jre)
        if not java_launcher:
            parser.error("Invalid JRE path: " + args.jre)
            return 1
    else:
        # Auto-detect the prebuilt bazel JRE first
        prebuilt_bazel_jdk = os.path.join(
            _FUCHSIA_DIR,
            "prebuilt",
            "third_party",
            "bazel",
            build_utils.get_host_tag(),
            "install_base",
            "embedded_tools",
            "jdk",
        )
        java_launcher = find_java_binary(prebuilt_bazel_jdk)
        if not java_launcher:
            print(
                "ERROR: Missing prebuilt Bazel JDK launcher, please use --jre=<DIR>: %s/%s"
                % (prebuilt_bazel_jdk, java_binary),
                file=sys.stderr,
            )
            return 1

    def verbose(msg: str) -> None:
        if args.verbose:
            print("DEBUG: " + msg, file=sys.stderr)

    # Find the parser JAR file now.
    if args.log_parser_jar:
        log_parser_jar = args.log_parser_jar
    else:
        log_parser_jar = os.path.join(
            _FUCHSIA_DIR,
            "prebuilt",
            "third_party",
            "bazel_workspacelogparser",
            "bazel_workspacelogparser.jar",
        )

    if not os.path.exists(log_parser_jar):
        parser.error("Missing parser file: " + log_parser_jar)

    verbose("Using jar file at: " + log_parser_jar)

    log_file = args.log_file
    if not args.log_file:
        log_file = find_default_log_file()
        if not log_file:
            print(
                "ERROR: Could not find default log file, please use --log_file=FILE",
                file=sys.stderr,
            )
            return 1

    verbose("Using log file at: " + log_file)

    cmd = [java_launcher, "-jar", log_parser_jar, "--log_path=" + log_file]
    cmd += ["--exclude_rule=" + rule for rule in args.exclude_rule]
    if args.output_path:
        cmd += ["--output_path=" + args.output_path]

    verbose("Running command: %s" % " ".join([shlex.quote(c) for c in cmd]))

    return subprocess.run(cmd).returncode


if __name__ == "__main__":
    sys.exit(main())
