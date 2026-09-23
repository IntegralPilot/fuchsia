# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import collections
import dataclasses
import json
import os
import sys
from pathlib import Path

_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, os.path.join(_SCRIPT_DIR, "../../build/bazel/scripts"))
from build_utils import BazelLauncher, NinjaRunner

# Set this to True to debug operations locally in this script.
_DEBUG = False

_SECONDARY_BUILD_DIR_PREFIX = "build/secondary/"


def debug_log(msg: str) -> None:
    """Log a message to stderr if _DEBUG is True.

    Note that for performance reasons, only call this when _DEBUG is True.
    This avoids un-needed string formatting operations in the usual case where
    the flag is False.
    """
    assert _DEBUG, "Do not call debug_log() directly, check for _DEBUG first!"
    print(msg, file=sys.stderr)


@dataclasses.dataclass(frozen=True)
class TestTargetInfo:
    """Description of a single tests.json entry.

    The format is documented at
    https://fuchsia.dev/fuchsia-src/reference/testing/tests-json-format
    """

    # For Bazel tests, begins with @ and relies on os_name to know which platform to build it.
    # Note that this is the label to use to build the test. When a Bazel test target is wrapped
    # by a bazel_test_package_group(), this label will be the label of the group, not the test,
    # which will appear in the "source_label" field, which is ignored here.
    label: str

    # The OS the test must run on. "linux" or "fuchsia".
    os_name: str

    # Used by host tests to point to the main test executable / script.
    path: str = ""

    # Used by device end-to-end tests to point to the host test runner executable.
    # See https://fxbug.dev/458823250.
    new_path: str = ""

    # A list of paths to package manifest files. Those are generated at build time,
    # so cannot be read directly, but they should be part of the affected artifacts when the
    # corresponding source file changes.
    package_manifests: list[str] = dataclasses.field(default_factory=list)

    # Points to a JSON file that contains an array of string paths to other package manifests,
    # corresponding to extra packages needed at runtime during testing. For GN tests, it is
    # generated at regeneration time, before the build, and is safe to read here. For Bazel tests
    # this is generated at build time, and cannot be read directly (the information must be
    # extracted with a query instead).
    package_manifest_deps: str = ""

    # Points to a JSON file that contains an array of string paths to Ninja artifacts needed at
    # runtime. For GN tests, it is generated at regeneration time, before the build, and is safe
    # to read here. For Bazel tests, this must be obtained with a query.
    runtime_deps: str = ""

    # The full GN label of the package for this test.
    package_label: str = ""


def gn_label_to_build_gn_path(label: str) -> str:
    """Return the relative path to the BUILD.gn file defining a GN label.

    For example:
      //src/foo:bar(//build/toolchain:arm64) -> src/foo/BUILD.gn
      //src/foo/bar:bar                      -> src/foo/bar/BUILD.gn
      //:root_target                         -> BUILD.gn
    """
    if not label or label.startswith("@"):
        return ""
    # Strip toolchain if present: //foo:bar(//build/toolchain:...)
    target = label.partition("(")[0]
    # Strip // prefix
    if target.startswith("//"):
        target = target[2:]
    # Strip target name after :
    pkg_dir = target.partition(":")[0]
    if pkg_dir:
        return os.path.normpath(os.path.join(pkg_dir, "BUILD.gn"))
    return "BUILD.gn"


def parse_tests_json(build_dir: Path) -> list[TestTargetInfo]:
    """Parse the tests.json file and return a list of TestTargetInfo values.

    Args:
        build_dir: Path to Ninja build directory.
    Returns:
        A list of TestTargetInfo values.
    """
    result: list[TestTargetInfo] = []

    tests_json_path = build_dir / "tests.json"
    with tests_json_path.open("rt") as f:
        tests_json = json.load(f)

    for entry in tests_json:
        test = entry["test"]
        test_label = test["label"]
        test_os = test["os"]

        package_label = test.get("package_label", "")
        if package_label:
            assert isinstance(package_label, str)

        path = test.get("path", "")
        if path:
            assert isinstance(path, str)

        new_path = test.get("new_path", "")
        if new_path:
            assert isinstance(new_path, str)

        package_manifests = test.get("package_manifests", [])
        if package_manifests:
            assert isinstance(package_manifests, list)

        package_manifest_deps_path = test.get("package_manifest_deps", "")
        if package_manifest_deps_path:
            assert isinstance(package_manifest_deps_path, str)

        runtime_deps_path = test.get("runtime_deps", "")
        if runtime_deps_path:
            assert isinstance(runtime_deps_path, str)

        result.append(
            TestTargetInfo(
                label=test_label,
                os_name=test_os,
                path=path,
                new_path=new_path,
                package_manifests=package_manifests,
                package_manifest_deps=package_manifest_deps_path,
                runtime_deps=runtime_deps_path,
                package_label=package_label,
            )
        )

    return result


def split_gn_and_bazel_tests(
    input_tests: list[TestTargetInfo],
) -> tuple[list[TestTargetInfo], list[TestTargetInfo]]:
    """Split a list of TestTargetInfo into GN and Bazel tests.

    Args:
        input_tests: List of TestTargetInfo values.
    Returns:
        A tuple of (gn_tests, bazel_tests).
    """
    gn_tests: list[TestTargetInfo] = []
    bazel_tests: list[TestTargetInfo] = []
    for test in input_tests:
        if test.label.startswith("@"):
            bazel_tests.append(test)
        else:
            gn_tests.append(test)
    return gn_tests, bazel_tests


@dataclasses.dataclass(frozen=True)
class GnTestArtifactsInfo:
    # Name of the os this test must run on. "linux" or "fuchsia".
    os_name: str

    # Set of Ninja artifact paths, relative to the Ninja build directory
    ninja_artifacts: set[str]


class GnTestArtifactsMap(dict[str, GnTestArtifactsInfo]):
    """A mapping from GN test labels to their GnTestArtifactsInfo value."""


def create_gn_test_artifacts_mapping(build_dir: Path) -> GnTestArtifactsMap:
    gn_test_infos, bazel_test_infos = split_gn_and_bazel_tests(
        parse_tests_json(build_dir)
    )
    return _create_gn_test_artifacts_mapping(gn_test_infos, build_dir)


def _create_gn_test_artifacts_mapping(
    gn_test_infos: list[TestTargetInfo],
    build_dir: Path,
) -> GnTestArtifactsMap:
    """Generate a mapping from GN test labels to their OS and Ninja artifacts.

    Args:
        gn_test_infos: List of GN test targets.
        build_dir: Ninja build directory.
    Returns:
        A GnTestTargetMap value. The first element is the OS of the test (e.g. 'fuchsia' or 'linux').
        The second element is a set of Ninja artifact paths, relative to the Ninja
        build directory, that each test target should produce or use at
        runtime.
    """
    result = GnTestArtifactsMap()

    for test in gn_test_infos:
        target_label = test.label
        assert not target_label.startswith(
            "@@"
        ), f"Unexpected Bazel test label {target_label}"

        test_os = test.os_name

        # It is common to get duplicates because runtime_deps and package_manifest_deps
        # are the result of GN metadata collection, which does simple concatenation of
        # lists instead of unions of sets.
        artifacts: set[str] = set()

        if test.path:
            artifacts.add(test.path)

        if test.new_path:
            artifacts.add(test.new_path)

        # test.package_manifests are generated at build time, while they cannot be read directly,
        # they should be part of the affected artifacts when the corresponding source file changes.
        if test.package_manifests:
            artifacts.update(test.package_manifests)

        # test.package_manifest_deps is generated at regeneration time for GN tests, and is safe
        # to read here.
        if test.package_manifest_deps:
            with (build_dir / test.package_manifest_deps).open("rt") as f:
                manifests = json.load(f)
                assert isinstance(manifests, list)
                artifacts.update(manifests)
            artifacts.add(test.package_manifest_deps)

        # test.runtime_deps is generated at regeneration time for GN tests, and is safe
        # to read here. It points to a JSON file that contains an array of
        # string paths to Ninja artifacts needed at runtime.
        if test.runtime_deps:
            with (build_dir / test.runtime_deps).open("rt") as f:
                runtime_deps = json.load(f)
                assert isinstance(runtime_deps, list)
                artifacts.update(runtime_deps)
            artifacts.add(test.runtime_deps)

        result[target_label] = GnTestArtifactsInfo(
            os_name=test_os, ninja_artifacts=artifacts
        )

    return result


@dataclasses.dataclass(frozen=True)
class AffectedTestTarget:
    """Represents a test target and its operating system."""

    # The test label. GN test labels begin with // and always contain a toolchain suffix,
    # while Bazel test labels begin with @ and rely on os_name to know which platform to use
    # to build them.
    label: str
    os_name: str


def map_file_path_to_bazel_label(file_path: str, fuchsia_dir: Path) -> str:
    """Map a given file path to a Bazel target label.

    This inspects the filesystem to find package boundaries determined
    by the existence of BUILD.bazel files.

    For example, is //some/package/BUILD.bazel exists, then an input
    of 'some/package/with/target/in/subdir' will produce a result
    of '@@//some/package:with/target/in/subdir'.

    Args:
        file_path: A file path. If relative, this is assumed to be relative to
            fuchsia_dir.
        fuchsia_dir: The path to the Fuchsia source directory.
    Returns:
        A Bazel target label looking like @@//<package>:<target>.
    """
    if os.path.isabs(file_path):
        file_path = os.path.relpath(file_path, fuchsia_dir)
    # Need to find the BUILD.bazel file that defines a package covering this source file.
    # This could be cached for performance.
    package_path = os.path.dirname(file_path)
    while package_path != "":
        if (fuchsia_dir / package_path / "BUILD.bazel").exists():
            break
        package_path = os.path.dirname(package_path)

    if package_path == ".":
        package_path = ""

    return f"@@//{package_path}:{os.path.relpath(file_path, package_path)}"


def map_file_paths_to_bazel_labels(
    file_paths: set[str], fuchsia_dir: Path
) -> set[str]:
    """Convert a list of source files, relative to the Fuchsia directory, into a set of Bazel labels.

    Args:
        file_paths: An iterable of source files, relative to the Fuchsia directory.
        fuchsia_dir: The path to the Fuchsia source directory.
    Returns:
        A set of Bazel labels corresponding to the input source files.
    """
    return {
        map_file_path_to_bazel_label(file_path, fuchsia_dir)
        for file_path in file_paths
    }


def find_bazel_tests_affected_by_changed_files(
    changed_files: set[str],
    bazel_tests: list[TestTargetInfo],
    fuchsia_dir: Path,
    ninja_build_dir: Path,
    bazel_launcher: BazelLauncher,
) -> list[AffectedTestTarget]:
    """Extract the list of Bazel test targets from tests.json

    Args:
        changed_files: A list of changed source files, relative to the Fuchsia source directory.
        bazel_tests: A list of TestTargetInfo for Bazel-defined tests.
        fuchsia_dir: A Path to the Fuchsia source directory.
        ninja_build_dir: Path to Ninja build directory.
        bazel_launcher: A BazelLauncher instance used to run queries.
    Returns:
        A list of AffectedTestTarget values.
    """
    # There are three sets of files to consider:
    #
    # - Regular input sources, these are passed as inputs to rdeps(), then the
    #   result is intersected with the labels of the test labels to find which
    #   ones are affected.
    #
    # - Bazel BUILD.bazel files, these are ignored as inputs by rdeps(), but one can
    #   substitute //src/foo:BUILD.bazel with //src/foo:all to get equivalent results.
    #
    # - Bazel .bzl files, these are ignored as inputs to query functions.
    #   However, it is possible to use buildfiles(deps(<target_set>)) to report the
    #   corresponding .bzl files, then match these with the changed .bzl files.
    #
    #   To avoid doing one query per test label, use binary partitioning to find
    #   the set of affected tests. This will still be significantly slower than
    #   the above two cases though.
    #
    all_test_labels = {test.label for test in bazel_tests}

    changed_input_labels: set[str] = set()
    changed_bzl_labels: set[str] = set()
    for changed_label in map_file_paths_to_bazel_labels(
        changed_files, fuchsia_dir
    ):
        package, colon, target = changed_label.partition(":")
        if colon is None:
            target = os.path.basename(package)
        if target.endswith(".bzl"):
            changed_bzl_labels.add(f"{package}:{target}")
        else:
            if target in ("BUILD", "BUILD.bazel"):
                target = "all"
            changed_input_labels.add(f"{package}:{target}")

    def run_query(query_args: list[str]) -> list[str]:
        query_args = ["--config=quiet", "--consistent_labels"] + query_args
        if _DEBUG:
            debug_log(f"BAZEL QUERY: {query_args}\n")
        ret = bazel_launcher.run_query("query", query_args, ignore_errors=True)
        return ret.stdout.splitlines()

    affected_test_labels: set[str] = set()

    if changed_input_labels:
        # For regular source files, and BUILD files, use rdeps() to get the set of
        # reverse dependencies, then intersect it with our set of known test labels.
        reverse_source_deps = set(
            run_query(
                [
                    "rdeps(//...,set({}))".format(
                        " ".join(sorted(changed_input_labels))
                    ),
                ]
            )
        )

        if _DEBUG:
            debug_log(
                "All reverse source deps:\n  {}\n".format(
                    "\n  ".join(label for label in sorted(reverse_source_deps))
                )
            )

        affected_source_test_labels = reverse_source_deps & all_test_labels
        affected_test_labels.update(affected_source_test_labels)

        if _DEBUG:
            debug_log(
                "All affected source test labels:\n  {}\n".format(
                    "\n  ".join(sorted(affected_source_test_labels))
                )
            )

    if changed_bzl_labels:
        if _DEBUG:
            debug_log(
                "CHANGED BZL LABELS:\n  {}\n".format(
                    "\n  ".join(sorted(changed_bzl_labels))
                )
            )
        # For .bzl files, use buildfiles() to get the set of load files needed
        # by a given set of test targets. To avoid doing one query per test, use
        # binary partitioning to find the minimal set of test targets that cover
        # all the changed .bzl files.
        partition_queue: list[list[str]] = []
        partition_queue.append(sorted(all_test_labels))
        while partition_queue:
            if _DEBUG:
                debug_log(
                    "PARTITION QUEUE {}:\n  {}\n".format(
                        len(partition_queue),
                        "\n  ".join(
                            f"{len(partition)} - {partition[0]}..."
                            for partition in partition_queue
                        ),
                    )
                )
            current_partition = partition_queue.pop(0)

            # Find the .bzl files required by this partition, then intersect
            # them with changed_bzl_files.
            build_labels = run_query(
                [
                    "buildfiles(deps(set({})))".format(
                        " ".join(sorted(current_partition))
                    ),
                ]
            )
            bzl_labels = set(f for f in build_labels if f.endswith(".bzl"))
            affected_bzl_labels = bzl_labels & changed_bzl_labels
            if _DEBUG:
                debug_log(
                    "AFFECTED BZL LABELS:\n  {}".format(
                        "\n  ".join(
                            label for label in sorted(affected_bzl_labels)
                        )
                    )
                )
            if not affected_bzl_labels:
                # No .bzl files in this partition are affected, so skip it.
                continue

            if len(current_partition) == 1:
                # Found an individual test affected by the changed .bzl files.
                affected_test_labels.add(current_partition[0])
                continue

            # Split the partition in half and add one half to the queue,
            # process the other in this loop iteration.
            mid = len(current_partition) // 2
            partition_queue.append(current_partition[mid:])
            partition_queue.append(current_partition[:mid])

    label_to_os_names = collections.defaultdict(list)
    for test in bazel_tests:
        label_to_os_names[test.label].append(test.os_name)

    result: list[AffectedTestTarget] = []
    for label in affected_test_labels:
        for os_name in label_to_os_names[label]:
            result.append(AffectedTestTarget(label, os_name))

    return result


def _normalize_build_gn_path(path: str) -> str:
    """Normalize a path to a BUILD.gn file, stripping secondary tree prefixes."""
    normalized = os.path.normpath(path)
    if normalized.startswith(_SECONDARY_BUILD_DIR_PREFIX):
        return normalized[len(_SECONDARY_BUILD_DIR_PREFIX) :]
    return normalized


def _find_gn_tests_affected_by_build_gn_files(
    gn_tests: list[TestTargetInfo],
    changed_sources: set[str],
) -> set[AffectedTestTarget]:
    """Find GN tests whose BUILD.gn file was directly changed.

    In Ninja's graph, BUILD.gn is an input to GN regeneration (build.ninja.stamp),
    not directly to test targets, so ninja -t affected only reports build.ninja.stamp.
    This explicitly associates changed BUILD.gn files with the tests whose target
    or package is defined within them.
    """
    changed_build_gns = {
        _normalize_build_gn_path(source)
        for source in changed_sources
        if os.path.basename(source) == "BUILD.gn"
    }
    if not changed_build_gns:
        return set()

    return {
        AffectedTestTarget(label=test.label, os_name=test.os_name)
        for test in gn_tests
        if (
            gn_label_to_build_gn_path(test.label) in changed_build_gns
            or gn_label_to_build_gn_path(test.package_label)
            in changed_build_gns
        )
    }


def find_tests_affected_by_changed_files(
    changed_files: list[str],
    fuchsia_dir: Path,
    ninja_runner: NinjaRunner,
    bazel_launcher: BazelLauncher,
) -> set[AffectedTestTarget]:
    """Return the set of test labels that are affected by a set of changed files.

    Given a set of paths to changed files (for example after applying a
    git commit just after the last build), determine which targets need to
    be rebuilt (and for tests re-run), return the set of tests labels that
    would need to be rebuilt and then re-run after the build.

    Args:
        changed_files: List of file path strings, relative to Fuchsia source directory,
            of files that were changed since the last build.
        fuchsia_dir: Path to Fuchsia source directory.
        ninja_runner: A NinjaRunner instance.
        bazel_launcher: A BazelLauncher instance.
    Returns:
        A set of tuples, each containing a test target label and its OS name.
    """

    if _DEBUG:
        debug_log(f"changed_files={changed_files}")

    changed_sources: set[str] = set()
    for file in changed_files:
        if os.path.isabs(file):
            changed_sources.add(os.path.relpath(file, fuchsia_dir))
        else:
            changed_sources.add(str(file))

    build_dir = ninja_runner.build_dir

    gn_tests, bazel_tests = split_gn_and_bazel_tests(
        parse_tests_json(build_dir)
    )

    if _DEBUG:
        debug_log(
            "GN_TESTS: {}\n  ".format(
                "\n  ".join(test.label for test in gn_tests)
            ),
        )
        debug_log(
            "BAZEL_TESTS: {}\n  ".format(
                "\n  ".join(test.label for test in bazel_tests)
            ),
        )

    ninja_results: set[AffectedTestTarget] = set()

    if gn_tests:
        ninja_results.update(
            _find_gn_tests_affected_by_build_gn_files(gn_tests, changed_sources)
        )

        # Read the content of tests.json to determine which important artifacts
        # each test requires at runtime.
        gn_test_artifacts = _create_gn_test_artifacts_mapping(
            gn_tests, build_dir
        )

        # The list of source files as they must appear in the Ninja build plan.
        # All source inputs appear with a prefix like ../../ that corresponds
        # to the relative path from the build directory to the Fuchsia source one.
        source_prefix = os.path.relpath(fuchsia_dir, build_dir) + "/"
        ninja_sources = [
            f"{source_prefix}{source_path}"
            for source_path in sorted(changed_sources)
        ]

        # Run the 'affected' tool which returns the list of Ninja artifacts affected
        # by the changed sources. --ignore-errors is used because the changed file list
        # might include things that are not build plan inputs and should be ignored.
        # --depfile is used to ensure that implicit dependencies from the last build are
        # followed properly. This is critical for Bazel defined targets that are built
        # through bazel_action() GN target definitions.
        #
        # Note that for now, all Bazel targets, tests or not, must be wrapped through
        # GN bazel_action() targets.
        tool_output = ninja_runner.run_and_extract_output(
            [
                "-t",
                "affected",
                "--depfile",
                "--ignore-errors",
            ]
            + ninja_sources
        )

        affected_ninja_artifacts = set(tool_output.splitlines())

        ninja_results.update(
            {
                AffectedTestTarget(label=test_label, os_name=test_info.os_name)
                for test_label, test_info in gn_test_artifacts.items()
                if bool(test_info.ninja_artifacts & affected_ninja_artifacts)
            }
        )

    bazel_results: set[AffectedTestTarget] = set()

    if bazel_tests:
        bazel_results = set(
            find_bazel_tests_affected_by_changed_files(
                changed_sources,
                bazel_tests,
                fuchsia_dir,
                build_dir,
                bazel_launcher,
            )
        )

    return ninja_results | bazel_results
