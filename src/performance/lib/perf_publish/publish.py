# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import logging
import os
import pathlib
import re
import stat
import subprocess
import time
import types
from collections.abc import Collection, Sequence, Set
from importlib.resources import as_file, files
from typing import Any, Iterable, Self

from perf_publish import data  # type: ignore[attr-defined]
from perf_publish import metrics_allowlist, summarize

_LOGGER: logging.Logger = logging.getLogger(__name__)

# The 'test_suite' field should be all lower case.  It should start with 'fuchsia.', to distinguish
# Fuchsia test results from results from other projects that upload to Catapult (Chromeperf),
# because the namespace is shared between projects and Catapult does not enforce any separation
# between projects.
_TEST_SUITE_REGEX: re.Pattern[str] = re.compile(
    r"^fuchsia\.([a-z0-9_-]+\.)*[a-z0-9_-]+$"
)

# The regexp for the 'label' field is fairly permissive. This reflects what is currently generated
# by tests.
_LABEL_REGEX: re.Pattern[str] = re.compile(r"^[A-Za-z0-9_/.:=+<>\\ -]+$")

_FUCHSIA_PERF_EXT: str = ".fuchsiaperf.json"
_FUCHSIA_PERF_FULL_EXT: str = ".fuchsiaperf_full.json"
_CATAPULT_UPLOAD_ENABLED_EXT: str = ".catapult_json"
_CATAPULT_UPLOAD_DISABLED_EXT: str = ".catapult_json_disabled"
_SUMMARIZED_RESULTS_FILE: str = f"results{_FUCHSIA_PERF_EXT}"

ENV_CATAPULT_DASHBOARD_MASTER: str = "CATAPULT_DASHBOARD_MASTER"
ENV_CATAPULT_DASHBOARD_BOT: str = "CATAPULT_DASHBOARD_BOT"
ENV_BUILDBUCKET_ID: str = "BUILDBUCKET_ID"
ENV_BUILD_CREATE_TIME: str = "BUILD_CREATE_TIME"
ENV_RELEASE_VERSION: str = "RELEASE_VERSION"
ENV_FUCHSIA_EXPECTED_METRIC_NAMES_DEST_DIR: str = (
    "FUCHSIA_EXPECTED_METRIC_NAMES_DEST_DIR"
)
ENV_INTEGRATION_INTERNAL_GIT_COMMIT: str = "INTEGRATION_INTERNAL_GIT_COMMIT"
ENV_INTEGRATION_PUBLIC_GIT_COMMIT: str = "INTEGRATION_PUBLIC_GIT_COMMIT"
ENV_SMART_INTEGRATION_GIT_COMMIT: str = "SMART_INTEGRATION_GIT_COMMIT"


def publish_fuchsiaperf(
    fuchsia_perf_file_paths: Iterable[str | os.PathLike[str]],
    expected_metric_names_filename: str | os.PathLike[str],
    test_data_module: types.ModuleType | None = None,
    env: dict[str, str] = dict(os.environ),
    runtime_deps_dir: str | os.PathLike[str] | None = None,
) -> None:
    """Publishes the given metrics.

    Args:
        fuchsia_perf_file_paths: paths to the fuchsiaperf.json files containing the metrics. These
            will be summarized into a single fuchsiaperf.json file.
        expected_metric_names_filename: file name or path to file containing
            expected metric names to validate the actual metrics against.
        test_data_module: Python module containing the expected metric names file as a data file.
        env: map holding the environment variables.
        runtime_deps_dir: directory in which to look for necessary dependencies such as the expected
             metric names file, catapult converter, etc. Defaults to the test runtime_deps dir.
    """
    converter = CatapultConverter.from_env(
        fuchsia_perf_file_paths,
        expected_metric_names_filename,
        test_data_module=test_data_module,
        env=env,
        runtime_deps_dir=runtime_deps_dir,
    )
    converter.run()


class CatapultConverter:
    def __init__(
        self,
        fuchsia_perf_file_paths: Iterable[str | os.PathLike[str]],
        expected_metric_names_filename: str | os.PathLike[str],
        test_data_module: types.ModuleType | None = None,
        master: str | None = None,
        bot: str | None = None,
        build_bucket_id: str | None = None,
        build_create_time: str | None = None,
        release_version: str | None = None,
        integration_internal_git_commit: str | None = None,
        integration_public_git_commit: str | None = None,
        smart_integration_git_commit: str | None = None,
        fuchsia_expected_metric_names_dest_dir: str | None = None,
        current_time: int | None = None,
        subprocess_check_call: Any = subprocess.check_call,
        runtime_deps_dir: str | os.PathLike[str] | None = None,
    ):
        """Creates a new catapult converter.

        Args:
            fuchsia_perf_file_paths:
                Paths to the fuchsiaperf.json files containing the metrics.
                These will be summarized into a single fuchsiaperf.json file.

            expected_metric_names_filename:
                File name or path to file containing expected metric names to
                validate the actual metrics against.

            test_data_module:
                Python module containing the expected metric names file as
                a data file.  This should be created by the build system
                using the data_packages mechanism.

            integration_internal_git_commit:
                The internal integration.git revision which produced these data

            integration_public_git_commit:
                The public integration.git revision which produced these data

            smart_integration_git_commit:
                The smart-integration.git revision which produced these data

            fuchsia_expected_metric_names_dest_dir:
                Directory to which expected metrics are written.

            current_time:
                The current time, useful for testing. Defaults to time.time.

            subprocess_check_call:
                Allows to execute a process raising an exception on error.
                Useful for testing. Defaults to subprocess.check_call.

            runtime_deps_dir:
                Directory in which to look for necessary dependencies such as
                the expected metric names file, catapult converter, etc.
                Defaults to the test runtime_deps dir.

        See //src/testing/catapult_converter/README.md for the rest of args.
        """
        self._release_version = release_version
        self._integration_internal_git_commit = integration_internal_git_commit
        self._integration_public_git_commit = integration_public_git_commit
        self._smart_integration_git_commit = smart_integration_git_commit
        self._subprocess_check_call = subprocess_check_call
        self._fuchsia_expected_metric_names_dest_dir = (
            fuchsia_expected_metric_names_dest_dir
        )
        if runtime_deps_dir:
            self._runtime_deps_dir = runtime_deps_dir
        else:
            self._runtime_deps_dir = get_associated_runtime_deps_dir(__file__)

        self._upload_enabled: bool = True
        if master is None and bot is None:
            _LOGGER.info(
                "CatapultConverter: Infra env vars are not set; treating as a local run."
            )
            self._bot: str = "local-bot"
            self._master: str = "local-master"
            self._log_url: str = "http://ci.example.com/build/300"
            self._timestamp: int = (
                int(current_time if current_time else time.time()) * 1000
            )
            # Disable uploading so that we don't accidentally upload with the placeholder values
            # set here.
            self._upload_enabled = False
        elif (
            master is not None
            and bot is not None
            and build_bucket_id is not None
            and build_create_time is not None
        ):
            self._bot = bot
            self._master = master
            self._log_url = f"https://ci.chromium.org/b/{build_bucket_id}"
            self._timestamp = int(build_create_time)
        else:
            raise ValueError(
                "Catapult-related infra env vars are not set consistently"
            )

        # These data may be produced from either the public integration, or smart integration, but
        # not both.
        if (
            integration_public_git_commit is not None
            and smart_integration_git_commit is not None
        ):
            raise ValueError(
                "Data should be optionally produced from either public "
                "integration or smart integration, but not both"
            )

        fuchsia_perf_file_paths = self._check_extension_and_relocate(
            fuchsia_perf_file_paths
        )

        _LOGGER.debug("Checking metrics naming")
        should_summarize: bool = self._check_fuchsia_perf_metrics_naming(
            expected_metric_names_filename,
            fuchsia_perf_file_paths,
            test_data_module=test_data_module,
            runtime_deps_dir=self._runtime_deps_dir,
        )

        self._results_path = os.path.join(
            os.path.dirname(fuchsia_perf_file_paths[0]),
            _SUMMARIZED_RESULTS_FILE,
        )
        if should_summarize:
            results = summarize.summarize_perf_files(fuchsia_perf_file_paths)
            assert not os.path.exists(self._results_path)
            with open(self._results_path, "w") as f:
                summarize.write_fuchsiaperf_json(f, results)
        else:
            if len(fuchsia_perf_file_paths) > 1:
                raise ValueError("Expected a single file when not summarizing")
            os.rename(fuchsia_perf_file_paths[0], self._results_path)

        catapult_extension = (
            _CATAPULT_UPLOAD_ENABLED_EXT
            if self._upload_enabled
            else _CATAPULT_UPLOAD_DISABLED_EXT
        )
        self._output_file: str = (
            self._results_path.removesuffix(_FUCHSIA_PERF_EXT)
            + catapult_extension
        )

    def _check_extension_and_relocate(
        self, fuchsia_perf_file_paths: Iterable[str | os.PathLike[str]]
    ) -> list[str]:
        perf_file_paths = list(map(str, fuchsia_perf_file_paths))
        if len(perf_file_paths) == 0:
            raise ValueError("Expected at least one fuchsiaperf.json file")
        files_with_wrong_ext = []
        files_to_rename = []
        paths = []

        for p in perf_file_paths:
            if p.endswith(_FUCHSIA_PERF_EXT):
                files_to_rename.append(p)
            elif p.endswith(_FUCHSIA_PERF_FULL_EXT):
                paths.append(p)
            else:
                files_with_wrong_ext.append(p)

        if files_with_wrong_ext:
            raise ValueError(
                f"The following files must end with {_FUCHSIA_PERF_FULL_EXT} or {_FUCHSIA_PERF_EXT}:"
                "\n- {}\n".format("\n- ".join(files_with_wrong_ext))
            )

        for file in files_to_rename:
            file_without_suffix = file.removesuffix(_FUCHSIA_PERF_EXT)
            new_file = f"{file_without_suffix}{_FUCHSIA_PERF_FULL_EXT}"
            assert not os.path.exists(new_file)
            os.rename(file, new_file)
            paths.append(new_file)

        return paths

    @classmethod
    def from_env(
        cls,
        fuchsia_perf_file_paths: Iterable[str | os.PathLike[str]],
        expected_metric_names_filename: str | os.PathLike[str],
        test_data_module: types.ModuleType | None = None,
        env: dict[str, str] = dict(os.environ),
        runtime_deps_dir: str | os.PathLike[str] | None = None,
        current_time: int | None = None,
        subprocess_check_call: Any = subprocess.check_call,
    ) -> Self:
        """Creates a new catapult converter using the environment variables.

        Args:
            fuchsia_perf_file_paths: paths to the fuchsiaperf.json files containing the metrics.
            expected_metric_names_filename: file name or path to file containing expected metric names to
            validate the actual metrics against.
            env: map holding the environment variables.
            test_data_module: Python module containing the expected metric names file as a data file.
            current_time: the current time, useful for testing. Defaults to time.time.
            runtime_deps_dir: directory in which to look for necessary dependencies such as the expected
                metric names file, catapult converter, etc. Defaults to the test runtime_deps dir.
            subprocess_check_call: allows to execute a process raising an exception on error.
                Useful for testing. Defaults to subprocess.check_call.
        """
        return cls(
            fuchsia_perf_file_paths,
            expected_metric_names_filename,
            test_data_module=test_data_module,
            master=env.get(ENV_CATAPULT_DASHBOARD_MASTER),
            bot=env.get(ENV_CATAPULT_DASHBOARD_BOT),
            build_bucket_id=env.get(ENV_BUILDBUCKET_ID),
            build_create_time=env.get(ENV_BUILD_CREATE_TIME),
            release_version=env.get(ENV_RELEASE_VERSION),
            integration_internal_git_commit=env.get(
                ENV_INTEGRATION_INTERNAL_GIT_COMMIT
            ),
            integration_public_git_commit=env.get(
                ENV_INTEGRATION_PUBLIC_GIT_COMMIT
            ),
            smart_integration_git_commit=env.get(
                ENV_SMART_INTEGRATION_GIT_COMMIT
            ),
            fuchsia_expected_metric_names_dest_dir=env.get(
                ENV_FUCHSIA_EXPECTED_METRIC_NAMES_DEST_DIR
            ),
            runtime_deps_dir=runtime_deps_dir,
            current_time=current_time,
            subprocess_check_call=subprocess_check_call,
        )

    def run(self) -> None:
        """Publishes the given metrics."""
        with as_file(files(data).joinpath("catapult_converter")) as f:
            f.chmod(f.stat().st_mode | stat.S_IEXEC)
            args = self._args()
            _LOGGER.info(f'Performance: Running {f} {" ".join(args)}')
            self._subprocess_check_call([str(f)] + args)
            _LOGGER.info(
                f"Conversion to catapult results format completed. Output file: {self._output_file}"
            )

    def _check_fuchsia_perf_metrics_naming(
        self,
        expected_metric_names_file: str | os.PathLike[str],
        input_files: Sequence[str],
        test_data_module: types.ModuleType | None,
        runtime_deps_dir: str | os.PathLike[str],
    ) -> bool:
        metrics = self._extract_perf_file_metrics(input_files)
        if self._fuchsia_expected_metric_names_dest_dir is None:
            # TODO(b/340319757): Remove this conditional and make the case
            # where test_data_module is passed the only supported case.
            # That can be done after all tests have been changed to use
            # this case.
            if test_data_module:
                assert isinstance(expected_metric_names_file, str)
                with as_file(
                    files(test_data_module).joinpath(expected_metric_names_file)
                ) as filepath:
                    metric_allowlist = metrics_allowlist.MetricsAllowlist(
                        filepath
                    )
            else:
                metric_allowlist = metrics_allowlist.MetricsAllowlist(
                    os.path.join(runtime_deps_dir, expected_metric_names_file)
                )
            metric_allowlist.check(metrics)
            return metric_allowlist.should_summarize
        else:
            self._write_expectation_file(
                metrics,
                expected_metric_names_file,
                self._fuchsia_expected_metric_names_dest_dir,
            )
            return True

    def _extract_perf_file_metrics(
        self, input_files: Sequence[str]
    ) -> Set[str]:
        entries: set[str] = set()
        for input_file in input_files:
            with open(input_file) as f:
                json_data: str = json.load(f)

            if not isinstance(json_data, list):
                raise ValueError("Top level fuchsiaperf node should be a list")

            errors: list[str] = []
            for entry in json_data:
                if not isinstance(entry, dict):
                    raise ValueError(
                        "Expected entries in fuchsiaperf list to be objects"
                    )
                if "test_suite" not in entry:
                    raise ValueError(
                        'Expected key "test_suite" in fuchsiaperf entry'
                    )
                if "label" not in entry:
                    raise ValueError(
                        'Expected key "label" in fuchsiaperf entry'
                    )

                test_suite: str = entry["test_suite"]
                if not re.match(_TEST_SUITE_REGEX, test_suite):
                    errors.append(
                        f'test_suite field "{test_suite}" does not match the pattern '
                        f'"{_TEST_SUITE_REGEX}"'
                    )
                    continue

                label: str = entry["label"]
                if not re.match(_LABEL_REGEX, label):
                    errors.append(
                        f'test_suite field {label} does not match the pattern "{_LABEL_REGEX}"'
                    )
                    continue

                entries.add(f"{test_suite}: {label}")
            if errors:
                errors_string = "\n".join(errors)
                raise ValueError(
                    "Some performance test metrics don't follow the naming conventions:\n"
                    f"{errors_string}"
                )
        return entries

    def _write_expectation_file(
        self,
        metrics: Collection[str],
        expected_metric_names_filename: str | os.PathLike[str],
        fuchsia_expected_metric_names_dest_dir: str,
    ) -> None:
        dest_file: str = os.path.join(
            fuchsia_expected_metric_names_dest_dir,
            os.path.basename(expected_metric_names_filename),
        )
        with open(dest_file, "w") as f:
            for metric in sorted(metrics):
                f.write(f"{metric}\n")

    def _args(self) -> list[str]:
        args: list[str] = [
            "--input",
            str(self._results_path),
            "--output",
            self._output_file,
            "--execution-timestamp-ms",
            str(self._timestamp),
            "--masters",
            self._master,
            "--log-url",
            self._log_url,
            "--bots",
            self._bot,
        ]

        if self._release_version is not None:
            args += ["--product-versions", self._release_version]

        if self._integration_internal_git_commit is not None:
            args += [
                "--integration-internal-git-commit",
                self._integration_internal_git_commit,
            ]

        if self._integration_public_git_commit is not None:
            args += [
                "--integration-public-git-commit",
                self._integration_public_git_commit,
            ]

        if self._smart_integration_git_commit is not None:
            args += [
                "--smart-integration-git-commit",
                self._smart_integration_git_commit,
            ]

        return args


def get_associated_runtime_deps_dir(
    search_dir: str | os.PathLike[str],
) -> os.PathLike[str]:
    """Return the directory that contains runtime dependencies.

    Args:
      search_dir: Absolute path to directory where runtime_deps dir is an
        ancestor of.

    Returns: Path to runtime_deps directory
    """
    cur_path: str = os.path.dirname(search_dir)
    while not os.path.isdir(os.path.join(cur_path, "runtime_deps")):
        cur_path = os.path.dirname(cur_path)
        if cur_path == "/":
            raise ValueError("Couldn't find required runtime_deps directory")
    return pathlib.Path(cur_path) / "runtime_deps"
