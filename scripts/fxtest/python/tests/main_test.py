# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import contextlib
import gzip
import io
import json
import os
import re
import shutil
import signal
import subprocess
import tempfile
import typing
import unittest
import unittest.mock as mock

import agents.agents as agents_lib
import async_utils.command as command
from parameterized import parameterized

import args
import environment
import event
import execution
import find_affected
import log
import main
import selection
import selection_types
import summary
import test_list_file
import tests_json_file

WARNING_LEVEL = event.MessageLevel.WARNING


class TestMainIntegration(unittest.IsolatedAsyncioTestCase):
    """Integration tests for the main entrypoint.

    These tests encapsulate several real-world invocations of fx test,
    with mocked dependencies.
    """

    ORIGINAL_HAS_ACTIVE_DEVICE = main.AsyncMain._has_active_device
    ORIGINAL_WAIT_FOR_REPOSITORY_REGISTRATION = (
        main.AsyncMain._wait_for_repository_registration
    )

    DEVICE_TESTS_IN_INPUT = 1
    HOST_TESTS_IN_INPUT = 4
    E2E_TESTS_IN_INPUT = 1
    TOTAL_TESTS_IN_INPUT = DEVICE_TESTS_IN_INPUT + HOST_TESTS_IN_INPUT
    TOTAL_NON_E2E_TESTS_IN_INPUT = TOTAL_TESTS_IN_INPUT - E2E_TESTS_IN_INPUT

    def setUp(self) -> None:
        # Set up a Fake fuchsia directory.
        self.fuchsia_dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.fuchsia_dir.cleanup)

        # Set up mocks
        self.mocks = []

        # Retain the real build dir, if one exists.
        real_fuchsia_dir = os.getenv("FUCHSIA_DIR")

        # Intercept environment and instantiate a new mock FUCHSIA_DIR.
        self.mocks.append(
            mock.patch(
                "os.environ",
                {"FUCHSIA_DIR": self.fuchsia_dir.name},
            )
        )
        for m in self.mocks:
            m.start()
            self.addCleanup(m.stop)

        # Correct for location of the test data files between coverage.py
        # script and how tests are run in-tree.
        cur_path = os.path.dirname(__file__)
        while not os.path.isdir(cur_path):
            cur_path = os.path.split(cur_path)[0]

        # We use an external program to handle fuzzy matching called "dldist".
        # Put the program in the correct location so that the main script can
        # find it.
        os.makedirs(os.path.join(self.fuchsia_dir.name, "bin"))
        dldist_path = os.path.join(self.fuchsia_dir.name, "bin", "dldist")
        if os.path.exists(os.path.join(cur_path, "bin", "dldist")):
            # This path is used when executing the python_host_test target.
            print("Using the local dldist for matching")
            shutil.copy(os.path.join(cur_path, "bin", "dldist"), dldist_path)
        else:
            # This path is used when running coverage.py.
            print("Trying to use FUCHSIA_DIR dldist for the test")
            assert real_fuchsia_dir is not None
            build_dir: str
            with open(os.path.join(real_fuchsia_dir, ".fx-build-dir")) as f:
                build_dir = os.path.join(real_fuchsia_dir, f.read().strip())
            print(build_dir)
            assert os.path.isdir(build_dir)
            shutil.copy(
                os.path.join(build_dir, "host-tools", "dldist"), dldist_path
            )

        self.test_data_path = os.path.join(cur_path, "test_data/build_output")

        self.assertTrue(
            os.path.isfile(os.path.join(self.test_data_path, "tests.json")),
            f"path was {self.test_data_path} for {__file__}",
        )
        self.assertTrue(
            os.path.isfile(os.path.join(self.test_data_path, "test-list.json")),
            f"path was {self.test_data_path} for {__file__}",
        )
        self.test_list_input = os.path.join(
            self.test_data_path, "test-list.json"
        )
        self.assertTrue(
            os.path.isfile(
                os.path.join(self.test_data_path, "package-repositories.json")
            ),
            f"path was {self.test_data_path} for {__file__}",
        )

        disabled_tests_source_file = os.path.join(
            self.test_data_path, "disabled_tests.json"
        )
        self.assertTrue(
            os.path.isfile(disabled_tests_source_file),
            f"path was {self.test_data_path} for {__file__}",
        )

        with open(
            os.path.join(self.fuchsia_dir.name, ".fx-build-dir"), "w"
        ) as f:
            f.write("out/default")

        self.out_dir = os.path.join(self.fuchsia_dir.name, "out/default")
        os.makedirs(self.out_dir)

        for name in [
            "tests.json",
            "test-list.json",
            "package-repositories.json",
            "package-targets.json",
            "all_package_manifests.list",
        ]:
            shutil.copy(
                os.path.join(self.test_data_path, name),
                os.path.join(self.out_dir, name),
            )
        self.package_target_file_path = os.path.join(
            self.out_dir, "package-targets.json"
        )

        # disabled_tests.json must be in place for e2e tests to pass.
        disabled_tests_dest_path = os.path.join(
            self.fuchsia_dir.name, "sdk", "ctf"
        )
        os.makedirs(disabled_tests_dest_path)
        shutil.copy(
            disabled_tests_source_file,
            os.path.join(disabled_tests_dest_path, "disabled_tests.json"),
        )

        # Simulate the generated package metadata to test merging.
        with open(
            os.path.join(self.out_dir, "package_manifests_from_metadata.list"),
            "w",
        ) as f:
            json.dump(
                {
                    "content": {
                        "manifests": [
                            "obj/foo/package_manifest.json",
                            "obj/bar/package_manifest.json",
                            "obj/baz/package_manifest.json",
                        ]
                    }
                },
                f,
            )

        self._mock_get_device_environment(
            environment.DeviceEnvironment(
                "localhost", "8080", "foo", "/foo.key"
            )
        )

        self._mock_has_active_device(True)
        self._mock_wait_for_repository_registration(True)
        self._mock_uuid4()

        # Provide hard-coded predictable test-list.json content rather than
        # actually running the generate_test_list program.
        self._mock_generate_test_list()

        return super().setUp()

    def _mock_run_commands_in_parallel(
        self, stdout: str, return_code: int = 0
    ) -> mock.MagicMock:
        m = mock.AsyncMock(
            return_value=[
                mock.MagicMock(stdout=stdout, return_code=return_code)
            ]
        )
        patch = mock.patch("main.run_commands_in_parallel", m)
        patch.start()
        self.addCleanup(patch.stop)
        return m

    def _mock_run_command(
        self,
        return_code: int,
        async_handler: (
            typing.Callable[[typing.Any, typing.Any], typing.Awaitable[None]]
            | None
        ) = None,
        stdout: str = "",
    ) -> mock.MagicMock:
        async def handler(
            *args: typing.Any, **kwargs: typing.Any
        ) -> typing.Any:
            if async_handler is not None:
                await async_handler(*args, **kwargs)
            return mock.MagicMock(
                return_code=return_code,
                stdout=stdout,
                stderr="",
                was_timeout=False,
            )

        m = mock.AsyncMock(side_effect=handler)
        patch = mock.patch.object(execution, "run_command", m)
        patch.start()
        self.addCleanup(patch.stop)

        patch2 = mock.patch.object(selection.execution, "run_command", m)
        patch2.start()
        self.addCleanup(patch2.stop)

        return m

    def _mock_generate_test_list(self) -> mock.MagicMock:
        test_list_entries = test_list_file.TestListFile.entries_from_file(
            self.test_list_input
        )
        m = mock.AsyncMock(return_value=test_list_entries)
        patch = mock.patch("main.AsyncMain._generate_test_list", m)
        patch.start()
        self.addCleanup(patch.stop)
        return m

    def _mock_subprocess_call(self, value: int) -> mock.MagicMock:
        m = mock.MagicMock(return_value=value)
        patch = mock.patch("main.subprocess.call", m)
        patch.start()
        self.addCleanup(patch.stop)
        return m

    def _mock_has_package_server_connected_to_device(self, value: bool) -> None:
        m = mock.AsyncMock(return_value=value)
        patch = mock.patch("main.has_package_server_connected_to_device", m)
        patch.start()
        self.addCleanup(patch.stop)

    def _mock_has_active_device(self, value: bool) -> None:
        m = mock.AsyncMock(return_value=value)
        patch = mock.patch("main.AsyncMain._has_active_device", m)
        patch.start()
        self.addCleanup(patch.stop)

    def _mock_uuid4(self, value: str = "test-uuid") -> mock.MagicMock:
        m = mock.MagicMock(return_value=value)
        patch = mock.patch("main.uuid.uuid4", m)
        patch.start()
        self.addCleanup(patch.stop)
        return m

    def _mock_wait_for_repository_registration(self, value: bool) -> None:
        m = mock.AsyncMock(return_value=value)
        patch = mock.patch(
            "main.AsyncMain._wait_for_repository_registration", m
        )
        patch.start()
        self.addCleanup(patch.stop)

    def _mock_enumerate_mobly_test(
        self, output: command.CommandOutput
    ) -> mock.MagicMock:
        m = mock.AsyncMock(return_value=output)
        patch = mock.patch(
            "main.execution.TestExecution.enumerate_mobly_test", m
        )
        patch.start()
        self.addCleanup(patch.stop)
        return m

    def _mock_enumerate_test_cases(
        self, output: command.CommandOutput
    ) -> mock.MagicMock:
        m = mock.AsyncMock(return_value=output)
        patch = mock.patch(
            "main.execution.TestExecution.enumerate_test_cases", m
        )
        patch.start()
        self.addCleanup(patch.stop)
        return m

    def _mock_has_tests_in_base(self, test_packages: list[str]) -> None:
        with open(os.path.join(self.out_dir, "base_packages.list"), "w") as f:
            json.dump(
                {
                    "content": {
                        "names": test_packages,
                    }
                },
                f,
            )

    def _make_call_args_prefix_set(
        self, call_list: mock._CallList
    ) -> set[tuple[str, ...]]:
        """Given a list of mock calls, turn them into a set of prefixes for comparison.

        For instance, if the mock call is ("fx", "run", "command") the output
        is: {
            ('fx',),
            ('fx', 'run'),
            ('fx', 'run', 'command'),
        }

        This can be used to check containment.

        Args:
            call_list (mock._CallList): Calls to process.

        Returns:
            set[list[typing.Any]]: Set of prefixes to calls.
        """
        ret: set[tuple[str, ...]] = set()
        for call in call_list:
            args, _ = call
            cur = []
            if args and isinstance(args[0], list):
                # Correct for subprocess.call using lists and not *args.
                args = args[0]
            for a in args:
                cur.append(a)
                ret.add(tuple(cur))

        return ret

    def _assert_ffx_test_has_args(
        self, call_list: mock._CallList, desired_args: list[str]
    ) -> None:
        """Verifies args were passed to "ffx test".

        Given a list of mock calls, verifies it includes a call to
        "ffx test run" which includes the given sequence of args."""

        for call in call_list:
            try:
                call_args = list(call.args)
                ffx_pos = call_args.index("ffx")
            except:
                continue
            if call_args[ffx_pos : ffx_pos + 3] != ["ffx", "test", "run"]:
                continue
            args_after_ffx_run = call_args[ffx_pos + 3 :]
            for index, item in enumerate(args_after_ffx_run):
                if (
                    desired_args
                    == args_after_ffx_run[index : index + len(desired_args)]
                ):
                    return
            self.fail(f"{desired_args} not found in {call_list}")

    def _mock_get_device_environment(
        self, env: environment.DeviceEnvironment
    ) -> mock.MagicMock:
        m = mock.AsyncMock(return_value=env)
        patch = mock.patch(
            "main.execution.get_device_environment_from_exec_env", m
        )
        patch.start()
        self.addCleanup(patch.stop)
        return m

    def assertIsSubset(
        self, subset: set[typing.Any], full: set[typing.Any]
    ) -> None:
        inter = full.intersection(subset)
        self.assertEqual(
            inter, subset, f"Full set was\n {self.prettyFormatPrefixes(full)}"
        )

    def prettyFormatPrefixes(self, vals: set[typing.Any]) -> str:
        return "\n ".join(map(lambda x: " ".join(x), sorted(vals)))

    async def test_dry_run(self) -> None:
        """Test a basic dry run of the command."""
        recorder = event.EventRecorder()
        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--dry"]), recorder=recorder
        )
        self.assertEqual(ret, 0)

        selection_events: list[event.TestSelectionPayload] = [
            e.payload.test_selections
            async for e in recorder.iter()
            if e.payload is not None and e.payload.test_selections is not None
        ]

        self.assertEqual(len(selection_events), 1)
        selection_event = selection_events[0]
        self.assertEqual(
            len(selection_event.selected), self.TOTAL_TESTS_IN_INPUT
        )

    @mock.patch("main.execution.run_command")
    async def test_add_affected_tests(
        self, mock_run_command: mock.AsyncMock
    ) -> None:
        """Tests that _add_affected_tests executes correct add-test commands and returns labels."""
        mock_run_command.return_value = mock.Mock(return_code=0)

        app = main.AsyncMain.__new__(main.AsyncMain)

        targets = [
            find_affected.AffectedTarget(
                "//src/sys:foo_test",
                ["core.x64"],
                "fx add-test //src/sys:foo_test",
                is_host=False,
            ),
            find_affected.AffectedTarget(
                "//src/sys:baz_test",
                ["core.x64"],
                "fx add-test //src/sys:baz_test",
                is_host=False,
            ),
            find_affected.AffectedTarget(
                "//src/sys:bar_test",
                ["core.x64"],
                "fx add-host-test //src/sys:bar_test",
                is_host=True,
            ),
        ]

        exec_env = mock.Mock()
        exec_env.fx_cmd_line.side_effect = lambda *args: ["/path/to/fx"] + list(
            args
        )

        recorder = mock.Mock()

        labels = await app._add_affected_tests(targets, exec_env, recorder)
        labels = labels or []

        self.assertEqual(len(labels), 3)
        self.assertIn("//src/sys:foo_test", labels)
        self.assertIn("//src/sys:baz_test", labels)
        self.assertIn("//src/sys:bar_test", labels)

        self.assertEqual(mock_run_command.call_count, 2)

        # Verify first call adding target tests in a single command
        first_call = mock_run_command.call_args_list[0]
        self.assertIn("add-test", first_call[0])
        self.assertIn("//src/sys:foo_test", first_call[0])
        self.assertIn("//src/sys:baz_test", first_call[0])

        # Verify second call adding host test in a single command
        second_call = mock_run_command.call_args_list[1]
        self.assertIn("add-host-test", second_call[0])
        self.assertIn("//src/sys:bar_test", second_call[0])

    @mock.patch("main.execution.run_command")
    async def test_add_affected_tests_failure(
        self, mock_run_command: mock.AsyncMock
    ) -> None:
        """Tests that _add_affected_tests returns None if run_command fails."""
        app = main.AsyncMain.__new__(main.AsyncMain)
        targets = [
            find_affected.AffectedTarget(
                "//src/sys:foo_test",
                ["core.x64"],
                "fx add-test //src/sys:foo_test",
            ),
        ]
        exec_env = mock.Mock()
        exec_env.fx_cmd_line.side_effect = lambda *args: ["/path/to/fx"] + list(
            args
        )
        recorder = mock.Mock()

        # Case 1: run_command returns None
        mock_run_command.return_value = None
        labels = await app._add_affected_tests(targets, exec_env, recorder)
        self.assertIsNone(labels)
        recorder.emit_warning_message.assert_called_once()

        # Case 2: run_command returns non-zero code
        mock_run_command.reset_mock()
        recorder.emit_warning_message.reset_mock()
        mock_run_command.return_value = mock.Mock(return_code=1)
        labels = await app._add_affected_tests(targets, exec_env, recorder)
        self.assertIsNone(labels)
        recorder.emit_warning_message.assert_called_once()

    async def test_fuzzy_dry_run(self) -> None:
        """Test a dry run of the command for fuzzy matching"""
        recorder = event.EventRecorder()
        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--dry", "--fuzzy=1", "foo_test"]),
            recorder=recorder,
        )
        self.assertEqual(ret, 0)

        selection_events: list[event.TestSelectionPayload] = [
            e.payload.test_selections
            async for e in recorder.iter()
            if e.payload is not None and e.payload.test_selections is not None
        ]

        self.assertEqual(len(selection_events), 1)
        selection_event = selection_events[0]
        self.assertEqual(len(selection_event.selected), 1)

    async def test_cancel_before_tests_run(self) -> None:
        """Test that SIGINT before tests start running immediately stops execution"""

        self._mock_run_command(0)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        ready_to_kill = asyncio.Event()

        # Make builds hang for a long time, signalling that we should
        # trigger a SIGINT at the point the build starts.
        async def build_handler(_: typing.Any) -> bool:
            ready_to_kill.set()
            await asyncio.sleep(3600)
            return False

        build_patch = mock.patch(
            "main.AsyncMain._do_build",
            mock.AsyncMock(side_effect=build_handler),
        )
        build_patch.start()
        self.addCleanup(build_patch.stop)

        recorder = event.EventRecorder()
        main_task = asyncio.Task(
            main.async_main_wrapper(
                args.parse_args(["--simple"]), recorder=recorder
            )
        )

        await ready_to_kill.wait()
        os.kill(os.getpid(), signal.SIGINT)

        ret = await main_task
        self.assertEqual(ret, 1)
        errors = {e.error async for e in recorder.iter() if e.error is not None}
        self.assertIsSubset({"Terminated due to interrupt"}, errors)

    async def test_cancel_tests_wraps_up(self) -> None:
        """Test that SIGINT while tests are running allows them to wrap up and prints output"""

        ready_to_kill = asyncio.Event()

        async def command_handler(
            *args: typing.Any, **kwargs: typing.Any
        ) -> None:
            if "ffx" in args:
                return
            event: asyncio.Event = kwargs.get("abort_signal")  # type: ignore
            assert event is not None
            ready_to_kill.set()
            await event.wait()

        _command_mock = self._mock_run_command(
            15, async_handler=command_handler
        )
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        recorder = event.EventRecorder()
        main_task = asyncio.Task(
            main.async_main_wrapper(
                args.parse_args(["--simple", "--no-build"]), recorder=recorder
            )
        )

        await ready_to_kill.wait()
        os.kill(os.getpid(), signal.SIGINT)

        ret = await main_task
        self.assertEqual(ret, 1)
        errors = {e.error async for e in recorder.iter() if e.error is not None}
        self.assertIsSubset({"Failed to run all tests"}, errors)

        aborted_cases = {
            (payload_event.status, payload_event.message)
            async for e in recorder.iter()
            if (payload := e.payload) is not None
            and (payload_event := payload.test_suite_ended) is not None
        }
        self.assertSetEqual(
            aborted_cases,
            {
                (
                    event.TestSuiteStatus.ABORTED,
                    "Test suite aborted due to user interrupt.",
                )
            },
        )

    async def test_double_sigint_cancels_everything(self) -> None:
        """Test that sending SIGINT twice cancels all tasks, no matter how long running"""

        ready_to_kill = asyncio.Event()

        async def command_handler(
            *args: typing.Any, **kwargs: typing.Any
        ) -> None:
            if "ffx" in args:
                return
            ready_to_kill.set()
            await asyncio.sleep(3600)

        _command_mock = self._mock_run_command(
            15, async_handler=command_handler
        )
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        recorder = event.EventRecorder()
        main_task = asyncio.Task(
            main.async_main_wrapper(
                args.parse_args(["--simple", "--no-build"]), recorder=recorder
            )
        )

        await ready_to_kill.wait()
        os.kill(os.getpid(), signal.SIGINT)
        await asyncio.sleep(0.5)
        os.kill(os.getpid(), signal.SIGINT)

        ret = await main_task
        self.assertEqual(ret, 1)

    @parameterized.expand(
        [
            (["--host"], HOST_TESTS_IN_INPUT - E2E_TESTS_IN_INPUT),
            (["--device"], DEVICE_TESTS_IN_INPUT),
            (["--only-e2e"], E2E_TESTS_IN_INPUT),
            # TODO(https://fxbug.dev/338667899): Enable when we determine how to handle opt-in e2e.
            # ([], TOTAL_NON_E2E_TESTS_IN_INPUT),
            (["--e2e"], TOTAL_TESTS_IN_INPUT),
        ]
    )
    async def test_selection_flags(
        self, extra_flags: list[str], expected_count: int
    ) -> None:
        """Test that the correct --device, --host, or --e2e tests are selected"""

        recorder = event.EventRecorder()
        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--dry"] + extra_flags),
            recorder=recorder,
        )
        self.assertEqual(ret, 0)

        selection_events: list[event.TestSelectionPayload] = [
            e.payload.test_selections
            async for e in recorder.iter()
            if e.payload is not None and e.payload.test_selections is not None
        ]

        self.assertEqual(len(selection_events), 1)
        selection_event = selection_events[0]
        self.assertEqual(len(selection_event.selected), expected_count)

    @parameterized.expand(
        [
            ("--use-package-hash", DEVICE_TESTS_IN_INPUT),
            ("--no-use-package-hash", 0),
        ]
    )
    async def test_use_package_hash(
        self, flag_name: str, expected_hash_matches: int
    ) -> None:
        """Test ?hash= is used only when --use-package-hash is set"""

        command_mock = self._mock_run_command(0)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--no-build"] + [flag_name])
        )
        self.assertEqual(ret, 0)

        call_prefixes = self._make_call_args_prefix_set(
            command_mock.call_args_list
        )

        self.assertIsSubset(
            {
                (
                    "fx",
                    "--dir",
                    self.out_dir,
                    "ffx",
                    "test",
                    "run",
                ),
            },
            call_prefixes,
        )

        hash_params_found: int = 0
        for prefix_list in call_prefixes:
            entry: str
            for entry in prefix_list:
                if "?hash=" in entry:
                    hash_params_found += 1

        self.assertEqual(
            hash_params_found,
            expected_hash_matches,
            f"Prefixes were\n{self.prettyFormatPrefixes(call_prefixes)}",
        )

    @parameterized.expand(
        [
            ("default suggestions", [], 6),
            ("custom suggestion count", ["--suggestion-count=10"], 10),
            ("suppress suggestions", ["--no-show-suggestions"], 0),
        ]
    )
    async def test_suggestions(
        self,
        _unused_name: str,
        extra_flags: list[str],
        expected_suggestion_count: int,
    ) -> None:
        """Test that targets are suggested when there are no test matches."""
        mocked_commands = self._mock_run_commands_in_parallel("No matches")
        ret = await main.async_main_wrapper(
            args.parse_args(
                ["--simple", "non_existent_test_does_not_match"] + extra_flags
            )
        )
        self.assertEqual(ret, 1)
        if expected_suggestion_count > 0:
            self.assertListEqual(
                mocked_commands.call_args[0][0],
                [
                    [
                        "fx",
                        "--dir",
                        self.out_dir,
                        "search-tests",
                        f"--max-results={expected_suggestion_count}",
                        "--no-color",
                        "non_existent_test_does_not_match",
                    ]
                ],
            )
        else:
            self.assertListEqual(mocked_commands.call_args_list, [])

        # TODO(b/295340412): Test that suggestions are suppressed.

    @parameterized.expand(
        [
            ("default package server behavior", [], True, True),
            (
                "override no temporary package server",
                ["--no-allow-temporary-package-server"],
                False,
                False,
            ),
            (
                "override allow temporary package server",
                ["--allow-temporary-package-server"],
                True,
                True,
            ),
        ]
    )
    async def test_missing_package_server(
        self,
        _unused_name: str,
        extra_flags: list[str],
        expect_pass: bool,
        expect_to_serve: bool,
    ) -> None:
        """Test different behaviors when a package server is missing"""
        serve_abort_signal: asyncio.Event | None = None

        async def command_handler(
            *args: typing.Any, **kwargs: typing.Any
        ) -> None:
            nonlocal serve_abort_signal
            if "serve" in args:
                serve_abort_signal = kwargs.get("abort_signal")

        command_mock = self._mock_run_command(0, async_handler=command_handler)
        subprocess_mock = self._mock_subprocess_call(0)
        self._mock_has_package_server_connected_to_device(False)
        self._mock_has_tests_in_base([])

        ret = await main.async_main_wrapper(
            args.parse_args(["--simple"] + extra_flags)
        )
        if expect_pass:
            self.assertEqual(ret, 0)
        else:
            self.assertNotEqual(ret, 0)

        call_prefixes = self._make_call_args_prefix_set(
            command_mock.call_args_list
        )

        call_prefixes.update(
            self._make_call_args_prefix_set(subprocess_mock.call_args_list)
        )

        if expect_to_serve:
            self.assertIsSubset(
                {
                    (
                        "fx",
                        "--dir",
                        self.out_dir,
                        "serve",
                        "-l",
                        "0",
                        "--name",
                        "fxtest-temp-test-uuid",
                    ),
                    (
                        "fx",
                        "--dir",
                        self.out_dir,
                        "ffx",
                        "target",
                        "repository",
                        "deregister",
                        "-r",
                        "fxtest-temp-test-uuid",
                    ),
                },
                call_prefixes,
            )
            self.assertIsNotNone(serve_abort_signal)
            self.assertTrue(serve_abort_signal.is_set())  # type: ignore
        else:
            self.assertNotIn(
                ("fx", "--dir", self.out_dir, "serve"), call_prefixes
            )

    async def test_missing_device_starts_emulator(self) -> None:
        """Test that an emulator is started and stopped when no device is connected."""
        emu_started = False
        emu_stopped = False

        async def handler(
            *args: typing.Any, **kwargs: typing.Any
        ) -> typing.Any:
            if "emu" in args and "start" in args:
                self.assertTrue(
                    any("emu.instance_dir=" in str(a) for a in args)
                )
                nonlocal emu_started
                emu_started = True
                return mock.MagicMock(
                    return_code=0, stdout="Started emu", stderr=""
                )
            if "emu" in args and "stop" in args:
                self.assertEqual(
                    os.environ.get("FUCHSIA_NODENAME"), "127.0.0.1:12345"
                )
                self.assertTrue(
                    any("emu.instance_dir=" in str(a) for a in args)
                )
                nonlocal emu_stopped
                emu_stopped = True
                return mock.MagicMock(
                    return_code=0, stdout="Stopped emu", stderr=""
                )
            if "target" in args and "wait" in args:
                self.assertIn("--target", args)
                self.assertIn("fuchsia-emulator", args)
            return mock.MagicMock(return_code=0, stdout="", stderr="")

        self._mock_has_active_device(False)
        self._mock_wait_for_repository_registration(True)
        mock_json = json.dumps(
            [
                {
                    "nodename": "fuchsia-emulator",
                    "addresses": [{"ip": "127.0.0.1", "ssh_port": 12345}],
                }
            ]
        )
        self._mock_run_command(0, async_handler=handler, stdout=mock_json)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        mock_tempdir = mock.MagicMock()
        mock_tempdir.name = "/tmp/fuchsia-emulator"
        with (
            mock.patch(
                "main.tempfile.TemporaryDirectory", return_value=mock_tempdir
            ),
            mock.patch.dict(os.environ, {}, clear=False),
        ):
            ret = await main.async_main_wrapper(
                args.parse_args(["--simple", "--no-build"])
            )
            self.assertEqual(ret, 0)
            self.assertTrue(emu_started)
            self.assertTrue(emu_stopped)

    async def test_resolve_target_ip(self) -> None:
        """Test _resolve_target_ip returns address string on success and None on failure."""
        app = main.AsyncMain.__new__(main.AsyncMain)
        app._recorder = mock.Mock()
        app._exec_env = mock.Mock()
        app._exec_env.fx_cmd_line.side_effect = lambda *args: list(args)

        mock_json = json.dumps(
            [
                {
                    "nodename": "my-emu",
                    "addresses": [{"ip": "127.0.0.1", "ssh_port": 8022}],
                }
            ]
        )
        self._mock_run_command(0, stdout=mock_json)
        addr = await app._resolve_target_ip("my-emu")
        self.assertEqual(addr, "127.0.0.1:8022")

        missing = await app._resolve_target_ip("other-emu")
        self.assertIsNone(missing)

    @mock.patch("main.execution.run_command")
    async def test_get_emu_start_timeout(
        self, mock_run_command: mock.AsyncMock
    ) -> None:
        """Test _get_emu_start_timeout covers success, non-zero return code, invalid string, and timeout."""
        app = main.AsyncMain.__new__(main.AsyncMain)
        app._recorder = mock.Mock()
        app._exec_env = mock.Mock()
        app._exec_env.fx_cmd_line.side_effect = lambda *args: list(args)

        config_args = ("--config", "emu.instance_dir=/tmp/fxtest-emu")

        # 1. Successful query returning configured float.
        mock_run_command.return_value = mock.MagicMock(
            return_code=0, stdout='  "120.0" \n'
        )
        timeout = await app._get_emu_start_timeout(config_args)
        self.assertEqual(timeout, 120.0)
        mock_run_command.assert_called_once_with(
            "ffx",
            *config_args,
            "config",
            "get",
            "emu.start.timeout",
            recorder=app._recorder,
            timeout=main._DEFAULT_PROBE_TIMEOUT_SECONDS,
        )

        # 2. Non-zero return code falling back to default.
        mock_run_command.reset_mock()
        mock_run_command.return_value = mock.MagicMock(
            return_code=1, stdout="120.0"
        )
        timeout = await app._get_emu_start_timeout(config_args)
        self.assertEqual(timeout, main._DEFAULT_EMU_START_TIMEOUT_SECONDS)

        # 3. Invalid string output falling back to default.
        mock_run_command.reset_mock()
        mock_run_command.return_value = mock.MagicMock(
            return_code=0, stdout="invalid-timeout"
        )
        timeout = await app._get_emu_start_timeout(config_args)
        self.assertEqual(timeout, main._DEFAULT_EMU_START_TIMEOUT_SECONDS)

        # 4. Command timeout returning None falling back to default.
        mock_run_command.reset_mock()
        mock_run_command.return_value = None
        timeout = await app._get_emu_start_timeout(config_args)
        self.assertEqual(timeout, main._DEFAULT_EMU_START_TIMEOUT_SECONDS)

    async def test_missing_package_merkle_hash_fails_before_emulator_start(
        self,
    ) -> None:
        """Test that missing package Merkle hash fails fast (ret == 1) before starting the emulator."""
        with open(self.package_target_file_path, "w") as f:
            f.write('{"signed": {"targets": {}}}')

        emu_started = False

        async def handler(*args: typing.Any, **kwargs: typing.Any) -> None:
            if "emu" in args and "start" in args:
                nonlocal emu_started
                emu_started = True

        self._mock_has_active_device(False)
        self._mock_wait_for_repository_registration(True)
        self._mock_run_command(0, async_handler=handler)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        recorder = event.EventRecorder()
        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--no-build"]),
            recorder=recorder,
        )
        self.assertEqual(ret, 1)
        self.assertFalse(emu_started)
        warnings = [
            e.payload.user_message.value
            for e in recorder._events
            if e.payload and e.payload.user_message
        ]
        self.assertTrue(
            any("Could not find a Merkle hash" in w for w in warnings)
        )

    async def test_no_use_package_hash_skips_validation(self) -> None:
        """Test that --no-use-package-hash skips Merkle validation and proceeds to start emulator."""
        with open(self.package_target_file_path, "w") as f:
            f.write('{"signed": {"targets": {}}}')

        emu_started = False
        emu_stopped = False

        async def handler(*args: typing.Any, **kwargs: typing.Any) -> None:
            if "emu" in args and "start" in args:
                nonlocal emu_started
                emu_started = True
            if "emu" in args and "stop" in args:
                nonlocal emu_stopped
                emu_stopped = True

        self._mock_has_active_device(False)
        self._mock_wait_for_repository_registration(True)
        mock_json = json.dumps(
            [
                {
                    "nodename": "fuchsia-emulator",
                    "addresses": [{"ip": "127.0.0.1", "ssh_port": 12345}],
                }
            ]
        )
        self._mock_run_command(0, async_handler=handler, stdout=mock_json)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        mock_tempdir = mock.MagicMock()
        mock_tempdir.name = "/tmp/fuchsia-emulator"
        with (
            mock.patch(
                "main.tempfile.TemporaryDirectory", return_value=mock_tempdir
            ),
            mock.patch.dict(os.environ, {}, clear=False),
        ):
            ret = await main.async_main_wrapper(
                args.parse_args(
                    ["--simple", "--no-build", "--no-use-package-hash"]
                )
            )
            self.assertEqual(ret, 0)
            self.assertTrue(emu_started)
            self.assertTrue(emu_stopped)

    async def test_host_only_tests_bypass_merkle_validation(self) -> None:
        """Test that host-only tests bypass package Merkle validation even if targets are empty."""
        with open(self.package_target_file_path, "w") as f:
            f.write('{"signed": {"targets": {}}}')

        emu_started = False

        async def handler(*args: typing.Any, **kwargs: typing.Any) -> None:
            if "emu" in args and "start" in args:
                nonlocal emu_started
                emu_started = True

        self._mock_has_active_device(False)
        self._mock_run_command(0, async_handler=handler)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--no-build", "--host"])
        )
        self.assertEqual(ret, 0)
        self.assertFalse(emu_started)

    async def test_no_allow_temporary_emulator_does_not_start_emulator(
        self,
    ) -> None:
        """Test that an emulator is NOT started when --no-allow-temporary-emulator is passed."""
        emu_started = False

        async def handler(
            *args: typing.Any, **kwargs: typing.Any
        ) -> typing.Any:
            if "target" in args and "list" in args:
                return mock.MagicMock(return_code=0, stdout="", stderr="")
            if "emu" in args and "start" in args:
                nonlocal emu_started
                emu_started = True
                return mock.MagicMock(
                    return_code=0, stdout="Started emu", stderr=""
                )
            return mock.MagicMock(return_code=0, stdout="", stderr="")

        self._mock_has_active_device(False)
        self._mock_wait_for_repository_registration(True)
        self._mock_run_command(0, async_handler=handler)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        ret = await main.async_main_wrapper(
            args.parse_args(
                ["--simple", "--no-build", "--no-allow-temporary-emulator"]
            )
        )
        self.assertEqual(ret, 0)
        self.assertFalse(emu_started)

    async def test_device_specified_via_env_does_not_start_emulator(
        self,
    ) -> None:
        """Test that an emulator is NOT started when a device is specified via FUCHSIA_NODENAME."""
        emu_started = False

        async def handler(
            *args: typing.Any, **kwargs: typing.Any
        ) -> typing.Any:
            if "target" in args and "list" in args:
                return mock.MagicMock(return_code=0, stdout="", stderr="")
            if "target" in args and "echo" in args:
                return mock.MagicMock(return_code=0, stdout="hello", stderr="")
            if "emu" in args and "start" in args:
                nonlocal emu_started
                emu_started = True
                return mock.MagicMock(
                    return_code=0, stdout="Started emu", stderr=""
                )
            return mock.MagicMock(return_code=0, stdout="", stderr="")

        self._mock_wait_for_repository_registration(True)
        self._mock_run_command(0, async_handler=handler)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        # Set environment variable
        with mock.patch.dict(os.environ, {"FUCHSIA_NODENAME": "test-device"}):
            ret = await main.async_main_wrapper(
                args.parse_args(["--simple", "--no-build"])
            )
            self.assertEqual(ret, 0)
            self.assertFalse(emu_started)

    async def test_device_specified_via_default_does_not_start_emulator(
        self,
    ) -> None:
        """Test that an emulator is NOT started when a default device is configured."""
        emu_started = False

        async def handler(
            *args: typing.Any, **kwargs: typing.Any
        ) -> typing.Any:
            if "target" in args and "list" in args:
                return mock.MagicMock(return_code=0, stdout="", stderr="")
            if "target" in args and "default" in args and "get" in args:
                return mock.MagicMock(
                    return_code=0, stdout="test-device", stderr=""
                )
            if "target" in args and "echo" in args:
                return mock.MagicMock(return_code=0, stdout="hello", stderr="")
            if "emu" in args and "start" in args:
                nonlocal emu_started
                emu_started = True
                return mock.MagicMock(
                    return_code=0, stdout="Started emu", stderr=""
                )
            return mock.MagicMock(return_code=0, stdout="", stderr="")

        self._mock_wait_for_repository_registration(True)
        self._mock_run_command(0, async_handler=handler)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--no-build"])
        )
        self.assertEqual(ret, 0)
        self.assertFalse(emu_started)

    async def test_no_active_device_spawns_emulator_and_sets_nodename(
        self,
    ) -> None:
        """Test that if there is no active device initially, an emulator is spawned
        and FUCHSIA_NODENAME is set to its address.
        """
        emu_started = False
        emu_stopped = False
        emu_nodename_during_stop = None

        async def handler(
            *args: typing.Any, **kwargs: typing.Any
        ) -> typing.Any:
            nonlocal emu_started, emu_stopped, emu_nodename_during_stop
            if "target" in args and "default" in args and "get" in args:
                return mock.MagicMock(
                    return_code=1, stdout="", stderr="", was_timeout=False
                )
            if "target" in args and "list" in args:
                if emu_started:
                    return mock.MagicMock(
                        return_code=0,
                        stdout=json.dumps(
                            [
                                {
                                    "nodename": "fuchsia-temp-emulator",
                                    "rcs_state": "Y",
                                    "addresses": [
                                        {"ip": "127.0.0.1", "ssh_port": 12345}
                                    ],
                                }
                            ]
                        ),
                        stderr="",
                        was_timeout=False,
                    )
                return mock.MagicMock(
                    return_code=0, stdout="[]", stderr="", was_timeout=False
                )
            if "emu" in args and "start" in args:
                emu_started = True
                return mock.MagicMock(
                    return_code=0,
                    stdout="Started emu",
                    stderr="",
                    was_timeout=False,
                )
            if "emu" in args and "stop" in args:
                emu_nodename_during_stop = os.environ.get("FUCHSIA_NODENAME")
                emu_stopped = True
                return mock.MagicMock(
                    return_code=0,
                    stdout="Stopped emu",
                    stderr="",
                    was_timeout=False,
                )
            return mock.MagicMock(
                return_code=0, stdout="", stderr="", was_timeout=False
            )

        self._mock_has_active_device(False)
        self._mock_wait_for_repository_registration(True)
        command_mock = self._mock_run_command(0)
        command_mock.side_effect = handler
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        mock_tempdir = mock.MagicMock()
        mock_tempdir.name = "/tmp/fuchsia-temp-emulator"
        with (
            mock.patch(
                "main.tempfile.TemporaryDirectory", return_value=mock_tempdir
            ),
            mock.patch.dict(os.environ, {}, clear=False),
        ):
            ret = await main.async_main_wrapper(
                args.parse_args(["--simple", "--no-build"])
            )
            self.assertEqual(ret, 0)
            self.assertTrue(emu_started)
            self.assertTrue(emu_stopped)
            self.assertEqual(emu_nodename_during_stop, "127.0.0.1:12345")

    async def test_one_active_device_sets_nodename(self) -> None:
        """Test that if there is exactly one active device connected,
        FUCHSIA_NODENAME is set to its name, and no emulator is started.
        """
        emu_started = False

        async def handler(
            *args: typing.Any, **kwargs: typing.Any
        ) -> typing.Any:
            nonlocal emu_started
            if "target" in args and "list" in args:
                return mock.MagicMock(
                    return_code=0,
                    stdout=json.dumps(
                        [
                            {
                                "nodename": "fuchsia-active-device",
                                "rcs_state": "Y",
                            }
                        ]
                    ),
                    stderr="",
                    was_timeout=False,
                )
            if "emu" in args and "start" in args:
                emu_started = True
                return mock.MagicMock(
                    return_code=0,
                    stdout="Started emu",
                    stderr="",
                    was_timeout=False,
                )
            return mock.MagicMock(
                return_code=0, stdout="", stderr="", was_timeout=False
            )

        self._mock_has_active_device(True)
        self._mock_wait_for_repository_registration(True)
        command_mock = self._mock_run_command(0)
        command_mock.side_effect = handler
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        with mock.patch.dict(os.environ, {}):
            ret = await main.async_main_wrapper(
                args.parse_args(["--simple", "--no-build"])
            )
            self.assertEqual(ret, 0)
            self.assertFalse(emu_started)
            self.assertEqual(
                os.environ.get("FUCHSIA_NODENAME"), "fuchsia-active-device"
            )

    async def test_list_command_starts_and_terminates_package_server(
        self,
    ) -> None:
        """Test that we start and terminate a package server for the list command"""

        serve_abort_signal: asyncio.Event | None = None

        async def command_handler(
            *args: typing.Any, **kwargs: typing.Any
        ) -> None:
            nonlocal serve_abort_signal
            if "serve" in args:
                serve_abort_signal = kwargs.get("abort_signal")

        command_mock = self._mock_run_command(0, async_handler=command_handler)
        self._mock_run_commands_in_parallel("foo::test")
        self._mock_has_package_server_connected_to_device(False)
        self._mock_has_tests_in_base([])

        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--no-build", "--list"])
        )
        self.assertEqual(ret, 0)

        call_prefixes = self._make_call_args_prefix_set(
            command_mock.call_args_list
        )

        self.assertIsSubset(
            {
                (
                    "fx",
                    "--dir",
                    self.out_dir,
                    "serve",
                    "-l",
                    "0",
                    "--name",
                    "fxtest-temp-test-uuid",
                )
            },
            call_prefixes,
        )
        self.assertIsNotNone(serve_abort_signal)
        self.assertTrue(serve_abort_signal.is_set())  # type: ignore

    async def test_package_server_termination_on_generation_error(self) -> None:
        """Test that we terminate the package server if generation fails"""

        serve_abort_signal: asyncio.Event | None = None

        async def command_handler(
            *args: typing.Any, **kwargs: typing.Any
        ) -> None:
            nonlocal serve_abort_signal
            if "serve" in args:
                serve_abort_signal = kwargs.get("abort_signal")

        command_mock = self._mock_run_command(0, async_handler=command_handler)
        self._mock_has_package_server_connected_to_device(False)
        self._mock_has_tests_in_base([])

        with mock.patch(
            "main.AsyncMain._generate_test_list",
            side_effect=ValueError("Generation failed"),
        ):
            ret = await main.async_main_wrapper(
                args.parse_args(["--simple", "--no-build"])
            )
            self.assertEqual(ret, 1)

        call_prefixes = self._make_call_args_prefix_set(
            command_mock.call_args_list
        )

        self.assertIsSubset(
            {
                (
                    "fx",
                    "--dir",
                    self.out_dir,
                    "serve",
                    "-l",
                    "0",
                    "--name",
                    "fxtest-temp-test-uuid",
                )
            },
            call_prefixes,
        )
        self.assertIsNotNone(serve_abort_signal)
        self.assertTrue(serve_abort_signal.is_set())  # type: ignore

    async def test_package_server_termination_when_deregister_fails(
        self,
    ) -> None:
        """Test that we terminate the package server even if deregister fails"""

        serve_abort_signal: asyncio.Event | None = None

        async def command_handler(
            *args: typing.Any, **kwargs: typing.Any
        ) -> None:
            nonlocal serve_abort_signal
            if "serve" in args:
                serve_abort_signal = kwargs.get("abort_signal")
            elif "deregister" in args:
                raise RuntimeError("Deregistration failed")

        command_mock = self._mock_run_command(0, async_handler=command_handler)
        self._mock_has_package_server_connected_to_device(False)
        self._mock_has_tests_in_base([])

        ret = await main.async_main_wrapper(
            args.parse_args(
                ["--simple", "--no-build", "--allow-temporary-package-server"]
            )
        )
        self.assertEqual(ret, 0)
        self.assertIsNotNone(serve_abort_signal)
        self.assertTrue(serve_abort_signal.is_set())  # type: ignore

    async def test_package_server_termination_when_deregister_returns_error(
        self,
    ) -> None:
        """Test that we terminate the package server even if deregister returns non-zero exit code"""

        serve_abort_signal: asyncio.Event | None = None

        async def command_side_effect(
            *args: typing.Any, **kwargs: typing.Any
        ) -> mock.MagicMock:
            nonlocal serve_abort_signal
            if "serve" in args:
                serve_abort_signal = kwargs.get("abort_signal")
                return mock.MagicMock(
                    return_code=0, stdout="", stderr="", was_timeout=False
                )
            if "deregister" in args:
                return mock.MagicMock(
                    return_code=1, stdout="", stderr="", was_timeout=False
                )
            return mock.MagicMock(
                return_code=0, stdout="", stderr="", was_timeout=False
            )

        m = mock.AsyncMock(side_effect=command_side_effect)
        patch = mock.patch.object(execution, "run_command", m)
        patch.start()
        self.addCleanup(patch.stop)
        patch2 = mock.patch.object(selection.execution, "run_command", m)
        patch2.start()
        self.addCleanup(patch2.stop)

        self._mock_has_package_server_connected_to_device(False)
        self._mock_has_tests_in_base([])

        recorder = event.EventRecorder()
        ret = await main.async_main_wrapper(
            args.parse_args(
                ["--simple", "--no-build", "--allow-temporary-package-server"]
            ),
            recorder=recorder,
        )
        self.assertEqual(ret, 0)
        self.assertIsNotNone(serve_abort_signal)
        self.assertTrue(serve_abort_signal.is_set())  # type: ignore
        warnings = [
            e.payload.user_message.value
            for e in recorder._events
            if e.payload and e.payload.user_message
        ]
        self.assertTrue(
            any(
                "Failed to deregister temporary package repository" in w
                for w in warnings
            )
        )

    async def test_teardown_resources_cleans_up_package_server_and_emulator(
        self,
    ) -> None:
        """Test _teardown_resources cancels package server task and tears down emulator."""
        app = main.AsyncMain.__new__(main.AsyncMain)
        recorder = mock.Mock()
        app._recorder = recorder
        app._teardown_emulator = mock.AsyncMock(return_value=True)

        package_server_event = asyncio.Event()
        package_server_task = asyncio.create_task(asyncio.sleep(100))
        app._package_server_event = package_server_event
        app._package_server_task = package_server_task
        app._emu_instance_dir = mock.MagicMock()

        await app._teardown_resources()

        self.assertTrue(package_server_event.is_set())
        self.assertTrue(package_server_task.done())
        self.assertIsNone(app._package_server_event)
        self.assertIsNone(app._package_server_task)
        app._teardown_emulator.assert_awaited_once()

    async def test_main_wrapper_calls_teardown_resources_on_exception(
        self,
    ) -> None:
        """Test AsyncMain.main guarantees _teardown_resources is called when _main_impl raises."""
        app = main.AsyncMain.__new__(main.AsyncMain)
        app._teardown_resources = mock.AsyncMock()
        app._main_impl = mock.AsyncMock(side_effect=RuntimeError("Boom"))

        with self.assertRaises(RuntimeError):
            await app.main()

        app._teardown_resources.assert_awaited_once()

    def test_fallback_stop_emulator_success(self) -> None:
        """Test _fallback_stop_emulator runs subprocess.run with timeout=15 and cleans up instance dir."""
        app = main.AsyncMain.__new__(main.AsyncMain)
        app._exec_env = mock.Mock()
        app._exec_env.fx_cmd_line.side_effect = lambda *args: list(args)
        mock_tempdir = mock.MagicMock()
        mock_tempdir.name = "/tmp/test-emu"
        app._emu_instance_dir = mock_tempdir

        with (
            mock.patch("main.subprocess.run") as mock_run,
            mock.patch.dict(os.environ, {"FUCHSIA_NODENAME": "test-node"}),
        ):
            app._fallback_stop_emulator()
            mock_run.assert_called_once_with(
                [
                    "ffx",
                    "--config",
                    "emu.instance_dir=/tmp/test-emu",
                    "emu",
                    "stop",
                    "test-emu",
                ],
                check=False,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=15,
            )
            mock_tempdir.cleanup.assert_called_once()
            self.assertIsNone(app._emu_instance_dir)
            self.assertNotIn("FUCHSIA_NODENAME", os.environ)

    def test_fallback_stop_emulator_timeout_and_error(self) -> None:
        """Test _fallback_stop_emulator cleans up even if subprocess times out and cleanup errors."""
        app = main.AsyncMain.__new__(main.AsyncMain)
        app._exec_env = mock.Mock()
        app._exec_env.fx_cmd_line.side_effect = lambda *args: list(args)
        mock_tempdir = mock.MagicMock()
        mock_tempdir.name = "/tmp/test-emu"
        mock_tempdir.cleanup.side_effect = OSError("Permission denied")
        app._emu_instance_dir = mock_tempdir

        with (
            mock.patch(
                "main.subprocess.run",
                side_effect=subprocess.TimeoutExpired(
                    cmd=["ffx", "emu", "stop"], timeout=15
                ),
            ),
            mock.patch.dict(os.environ, {"FUCHSIA_NODENAME": "test-node"}),
        ):
            app._fallback_stop_emulator()
            mock_tempdir.cleanup.assert_called_once()
            self.assertIsNone(app._emu_instance_dir)
            self.assertNotIn("FUCHSIA_NODENAME", os.environ)

    async def test_teardown_emulator_success(self) -> None:
        """Test _teardown_emulator stops emulator with timeout=30.0, unregisters fallback, and cleans up."""
        app = main.AsyncMain.__new__(main.AsyncMain)
        recorder = mock.Mock()
        app._recorder = recorder
        app._exec_env = mock.Mock()
        app._exec_env.fx_cmd_line.side_effect = lambda *args: list(args)
        mock_tempdir = mock.MagicMock()
        mock_tempdir.name = "/tmp/test-emu"
        app._emu_instance_dir = mock_tempdir

        with (
            mock.patch(
                "main.execution.run_command",
                return_value=mock.MagicMock(return_code=0),
            ) as mock_run,
            mock.patch("main.atexit.unregister") as mock_unregister,
            mock.patch.dict(os.environ, {"FUCHSIA_NODENAME": "test-node"}),
        ):
            result = await app._teardown_emulator()
            self.assertTrue(result)
            mock_run.assert_awaited_once_with(
                "ffx",
                "--config",
                "emu.instance_dir=/tmp/test-emu",
                "emu",
                "stop",
                "test-emu",
                recorder=recorder,
                timeout=30.0,
            )
            mock_unregister.assert_called_once_with(app._fallback_stop_emulator)
            mock_tempdir.cleanup.assert_called_once()
            self.assertIsNone(app._emu_instance_dir)
            self.assertNotIn("FUCHSIA_NODENAME", os.environ)

    async def test_teardown_emulator_on_exception(self) -> None:
        """Test _teardown_emulator performs cleanup even when run_command raises an error."""
        app = main.AsyncMain.__new__(main.AsyncMain)
        recorder = mock.Mock()
        app._recorder = recorder
        app._exec_env = mock.Mock()
        app._exec_env.fx_cmd_line.side_effect = lambda *args: list(args)
        mock_tempdir = mock.MagicMock()
        mock_tempdir.name = "/tmp/test-emu"
        app._emu_instance_dir = mock_tempdir

        with (
            mock.patch(
                "main.execution.run_command",
                side_effect=RuntimeError("Command failed"),
            ),
            mock.patch("main.atexit.unregister") as mock_unregister,
            mock.patch.dict(os.environ, {"FUCHSIA_NODENAME": "test-node"}),
        ):
            with self.assertRaises(RuntimeError):
                await app._teardown_emulator()
            mock_unregister.assert_called_once_with(app._fallback_stop_emulator)
            mock_tempdir.cleanup.assert_called_once()
            self.assertIsNone(app._emu_instance_dir)
            self.assertNotIn("FUCHSIA_NODENAME", os.environ)

    async def test_teardown_emulator_cleanup_exception(self) -> None:
        """Test _teardown_emulator logs warning and completes when cleanup raises an exception."""
        app = main.AsyncMain.__new__(main.AsyncMain)
        recorder = mock.Mock()
        app._recorder = recorder
        app._exec_env = mock.Mock()
        app._exec_env.fx_cmd_line.side_effect = lambda *args: list(args)
        mock_tempdir = mock.MagicMock()
        mock_tempdir.name = "/tmp/test-emu"
        mock_tempdir.cleanup.side_effect = OSError("Permission denied")
        app._emu_instance_dir = mock_tempdir

        with (
            mock.patch(
                "main.execution.run_command",
                return_value=mock.MagicMock(return_code=0),
            ),
            mock.patch("main.atexit.unregister") as mock_unregister,
            mock.patch.dict(os.environ, {"FUCHSIA_NODENAME": "test-node"}),
        ):
            result = await app._teardown_emulator()
            self.assertTrue(result)
            mock_unregister.assert_called_once_with(app._fallback_stop_emulator)
            mock_tempdir.cleanup.assert_called_once()
            recorder.emit_warning_message.assert_called_once()
            self.assertIsNone(app._emu_instance_dir)

    async def test_full_success(self) -> None:
        """Test that we can run all tests and report success"""

        command_mock = self._mock_run_command(0)
        subprocess_mock = self._mock_subprocess_call(0)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--allow-temporary-package-server"])
        )
        self.assertEqual(ret, 0)

        call_prefixes = self._make_call_args_prefix_set(
            command_mock.call_args_list
        )

        call_prefixes.update(
            self._make_call_args_prefix_set(subprocess_mock.call_args_list)
        )

        # Make sure we built, published, and ran the device test.
        self.assertIsSubset(
            {
                (
                    "fx",
                    "--dir",
                    self.out_dir,
                    "build",
                    "--default",
                    "//src/sys:foo_test_package",
                    "--toolchain=//build/toolchain/host:x64",
                    "//src/sys:bar_test",
                    "//src/sys:baz_test",
                    "//src/tests/end_to_end:example_e2e_test",
                    "--default",
                    "//build/images/updates",
                ),
                (
                    "fx",
                    "--dir",
                    self.out_dir,
                    "build",
                    "--host",
                    "--quiet",
                    "@@//build/bazel/rules/host_tests/tests/cc:static_test",
                ),
                (
                    "fx",
                    "--dir",
                    self.out_dir,
                    "ffx",
                    "repository",
                    "publish",
                ),
                (
                    "fx",
                    "--dir",
                    self.out_dir,
                    "ffx",
                    "test",
                    "run",
                ),
            },
            call_prefixes,
        )

        self.assertNotIn(("fx", "--dir", self.out_dir, "serve"), call_prefixes)

        # Make sure we properly exclude the "broken_case" and "bad_case"
        # and count an empty test case set as passing.
        self._assert_ffx_test_has_args(
            command_mock.call_args_list, ["--test-filter", "-broken_case"]
        )
        self._assert_ffx_test_has_args(
            command_mock.call_args_list, ["--test-filter", "-bad_case"]
        )
        self._assert_ffx_test_has_args(
            command_mock.call_args_list, ["--no-cases-equals-success"]
        )

        # Make sure we ran the host tests.
        self.assertTrue(any(["bar_test" in v[0] for v in call_prefixes]))
        self.assertTrue(any(["baz_test" in v[0] for v in call_prefixes]))

        # Try running again, but this time replay the previous execution.
        output = mock.MagicMock(wraps=io.StringIO())
        output.fileno = lambda: -1
        with contextlib.redirect_stdout(output):
            ret = await main.async_main_wrapper(
                args.parse_args(
                    [
                        "--simple",
                        "-q",
                        "--previous",
                        "replay",
                        "--replay-speed",
                        "5",
                    ]
                ),
                replay_mode=True,
            )
            self.assertEqual(0, ret)

        contents = list(map(str.strip, output.getvalue().split("\n")))
        contents_for_printing = "\n ".join(contents)
        self.assertTrue(
            {
                "PASSED: host_x64/bar_test",
                "PASSED: fuchsia-pkg://fuchsia.com/foo-test#meta/foo_test.cm",
                "PASSED: host_x64/baz_test",
                "SKIPPED: host_x64/example_e2e_test",
            }.issubset(set(contents)),
            f"Contents were:\n {contents_for_printing}",
        )

    async def test_build_e2e(self) -> None:
        """Test that we build an updates package for e2e tests"""

        command_mock = self._mock_run_command(0)
        subprocess_mock = self._mock_subprocess_call(0)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--only-e2e"])
        )
        self.assertEqual(ret, 0)

        call_prefixes = self._make_call_args_prefix_set(
            command_mock.call_args_list
        )
        call_prefixes.update(
            self._make_call_args_prefix_set(subprocess_mock.call_args_list)
        )

        # Make sure we built, published, and ran the device test.
        self.assertIsSubset(
            {
                (
                    "fx",
                    "--dir",
                    os.path.join(self.fuchsia_dir.name, "out/default"),
                    "build",
                    "--toolchain=//build/toolchain/host:x64",
                    "//src/tests/end_to_end:example_e2e_test",
                    "--default",
                    "//build/images/updates",
                ),
                (
                    "fx",
                    "--dir",
                    self.out_dir,
                    "ffx",
                    "repository",
                    "publish",
                ),
            },
            call_prefixes,
        )

        # Make sure we ran the host tests.
        self.assertTrue(
            any(["example_e2e_test" in v[0] for v in call_prefixes])
        )

    async def test_build_device_package_lists_only_when_no_merkle(self) -> None:
        """Test that we only build package lists for device tests that are missing a merkle hash"""

        with open(self.package_target_file_path, "w") as f:
            # Clear the target files so that this test does not have a merkle hash listed.
            # This will trigger rebuilding package lists.
            f.write('{"signed": {"targets": {}}}')

        async def build_handler(
            *args: typing.Any, **kwargs: typing.Any
        ) -> None:
            if "build" in args:
                shutil.copy(
                    os.path.join(self.test_data_path, "package-targets.json"),
                    self.package_target_file_path,
                )

        command_mock = self._mock_run_command(0, async_handler=build_handler)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--no-e2e", "--device"])
        )
        self.assertEqual(ret, 0)

        call_prefixes = self._make_call_args_prefix_set(
            command_mock.call_args_list
        )

        # Make sure we built, published, and ran the device test.
        self.assertIsSubset(
            {
                (
                    "fx",
                    "--dir",
                    os.path.join(self.fuchsia_dir.name, "out/default"),
                    "build",
                    "--default",
                    "//src/sys:foo_test_package",
                    "--default",
                    "//build/images/updates:package_lists",
                    "//build/images/updates:discoverable_manifests_from_metadata.list",
                    "//build/images/updates:prepare_publish",
                ),
                (
                    "fx",
                    "--dir",
                    self.out_dir,
                    "ffx",
                    "repository",
                    "publish",
                ),
            },
            call_prefixes,
        )

    async def test_no_build_package_lists_if_not_needed(self) -> None:
        """Test that we do not build package lists if all packages are present in the repository"""

        command_mock = self._mock_run_command(0)
        subprocess_mock = self._mock_subprocess_call(0)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--no-e2e", "--device"])
        )
        self.assertEqual(ret, 0)

        call_prefixes = self._make_call_args_prefix_set(
            command_mock.call_args_list
        )
        call_prefixes.update(
            self._make_call_args_prefix_set(subprocess_mock.call_args_list)
        )

        self.assertIsSubset(
            {
                (
                    "fx",
                    "--dir",
                    os.path.join(self.fuchsia_dir.name, "out/default"),
                    "build",
                    "--default",
                    "//src/sys:foo_test_package",
                ),
                (
                    "fx",
                    "--dir",
                    self.out_dir,
                    "ffx",
                    "repository",
                    "publish",
                ),
            },
            call_prefixes,
        )

        for prefix_list in call_prefixes:
            self.assertNotIn(
                "//build/images/updates:package_lists", prefix_list
            )

    async def test_build_empty_selections(self) -> None:
        """Test that building with no selected tests does not raise UnboundLocalError."""
        self._mock_run_command(0)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        app = main.AsyncMain.__new__(main.AsyncMain)
        app._recorder = event.EventRecorder()
        app._flags = args.parse_args(["--simple"])
        app._exec_env = environment.ExecutionEnvironment.initialize_from_args(
            app._flags
        )
        app._end_execution_request_event = asyncio.Event()
        empty_selections = selection_types.TestSelections(
            selected=[],
            selected_but_not_run=[],
            best_score={},
            group_matches=[],
            fuzzy_distance_threshold=0,
        )
        self.assertTrue(await app._do_build(empty_selections))

    @mock.patch("main.run_build")
    async def test_do_build_missing_packages_includes_updates_target(
        self, mock_build: mock.AsyncMock
    ) -> None:
        """Test that device tests with missing packages add lightweight package list targets."""
        mock_build.return_value = mock.MagicMock(return_code=0)
        self._mock_has_tests_in_base([])

        app = main.AsyncMain.__new__(main.AsyncMain)
        app._recorder = event.EventRecorder()
        app._flags = args.parse_args(["--simple"])
        app._exec_env = environment.ExecutionEnvironment.initialize_from_args(
            app._flags
        )
        app._end_execution_request_event = asyncio.Event()
        app._publish_packages = mock.AsyncMock()

        mock_test = test_list_file.Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(
                    name="fuchsia-pkg://fuchsia.com/my-missing-pkg#meta/my-test.cm",
                    label="//src/sys:my_test(//build/toolchain/fuchsia:x64)",
                    os="fuchsia",
                    package_url="fuchsia-pkg://fuchsia.com/my-missing-pkg#meta/my-test.cm",
                )
            )
        )

        device_selections = selection_types.TestSelections(
            selected=[mock_test],
            selected_but_not_run=[],
            best_score={},
            group_matches=[],
            fuzzy_distance_threshold=0,
        )

        with mock.patch(
            "package_repository.PackageRepository.from_env"
        ) as mock_repo:
            mock_repo.return_value = mock.MagicMock(name_to_merkle={})
            self.assertTrue(await app._do_build(device_selections))

        mock_build.assert_called_once()
        build_targets = mock_build.call_args[0][1]
        self.assertIn("//build/images/updates:package_lists", build_targets)
        self.assertIn(
            "//build/images/updates:discoverable_manifests_from_metadata.list",
            build_targets,
        )
        self.assertIn("//build/images/updates:prepare_publish", build_targets)
        self.assertNotIn("//build/images/updates", build_targets)

    async def test_no_build(self) -> None:
        """Test that we can run all tests and report success"""

        command_mock = self._mock_run_command(0)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--no-build"])
        )
        self.assertEqual(ret, 0)

        call_prefixes = self._make_call_args_prefix_set(
            command_mock.call_args_list
        )

        self.assertFalse(
            ("fx", "--dir", self.out_dir, "build") in call_prefixes
        )
        self.assertFalse(
            ("fx", "--dir", self.out_dir, "ffx", "repository", "publish")
            in call_prefixes
        )

        self.assertIsSubset(
            {
                ("fx", "--dir", self.out_dir, "ffx", "test", "run"),
            },
            call_prefixes,
        )

        # Make sure we ran the host test.
        self.assertTrue(any(["bar_test" in v[0] for v in call_prefixes]))
        self.assertTrue(any(["baz_test" in v[0] for v in call_prefixes]))

    async def test_first_failure(self) -> None:
        """Test that one failing test aborts the rest with --fail"""

        command_mock = self._mock_run_command(1)
        command_mock.side_effect = [
            command.CommandOutput("out", "err", 1, 10, None),
            command.CommandOutput("out", "err", 1, 10, None),
            command.CommandOutput("out", "err", 1, 10, None),
        ]

        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--no-build", "--fail"])
        )

        # bar_test and baz_test are not hermetic, so cannot run at the same time.
        # One of them will run before the other, which means --fail
        # prevents one of them from starting, and we expect to see
        # only bar_test (since baz_test is defined later in the file)
        call_prefixes = self._make_call_args_prefix_set(
            command_mock.call_args_list
        )
        self.assertEqual(ret, 1)

        self.assertTrue(any(["bar_test" in v[0] for v in call_prefixes]))
        self.assertFalse(any(["baz_test" in v[0] for v in call_prefixes]))

    async def test_abort_all_tests_event_triggered_with_fail(self) -> None:
        """Test that abort_all_tests_event is triggered by running multiple tests with the --fail flag."""
        foo_started = asyncio.Event()
        foo_abort_signal: asyncio.Event | None = None

        async def command_handler(
            *args: typing.Any, **kwargs: typing.Any
        ) -> mock.MagicMock:
            nonlocal foo_abort_signal
            command_str = " ".join(str(a) for a in args)
            if "foo-test" in command_str:
                foo_abort_signal = kwargs.get("abort_signal")
                assert foo_abort_signal is not None
                foo_started.set()
                # Wait until abort_signal is triggered by the failure of bar_test.
                await foo_abort_signal.wait()
                return mock.MagicMock(
                    return_code=0,
                    stdout="",
                    stderr="",
                    was_timeout=False,
                )
            elif "bar_test" in command_str:
                # Ensure foo-test has started running concurrently before bar_test fails.
                await foo_started.wait()
                return mock.MagicMock(
                    return_code=1,
                    stdout="bar_test failed",
                    stderr="",
                    was_timeout=False,
                )
            return mock.MagicMock(
                return_code=0,
                stdout="",
                stderr="",
                was_timeout=False,
            )

        command_mock = self._mock_run_command(0)
        command_mock.side_effect = command_handler
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        recorder = event.EventRecorder()
        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--no-build", "--fail"]),
            recorder=recorder,
        )

        self.assertEqual(ret, 1)

        # Verify that abort_all_tests_event was passed as abort_signal and was set.
        assert foo_abort_signal is not None
        self.assertTrue(foo_abort_signal.is_set())

        # foo-test and bar_test ran concurrently; baz_test was aborted before running.
        call_prefixes = self._make_call_args_prefix_set(
            command_mock.call_args_list
        )
        self.assertTrue(any(["bar_test" in v[0] for v in call_prefixes]))
        self.assertTrue(any(["foo-test" in " ".join(v) for v in call_prefixes]))
        self.assertFalse(any(["baz_test" in v[0] for v in call_prefixes]))

        # Verify test suite statuses from recorded events:
        # bar_test should have FAILED, and foo-test should have ABORTED due to the failure.
        suite_statuses = [
            (payload_event.status, payload_event.message)
            async for e in recorder.iter()
            if (payload := e.payload) is not None
            and (payload_event := payload.test_suite_ended) is not None
        ]
        self.assertIn(
            (
                event.TestSuiteStatus.FAILED,
                None,
            ),
            suite_statuses,
        )
        self.assertIn(
            (
                event.TestSuiteStatus.ABORTED,
                "Test suite aborted due to another failure",
            ),
            suite_statuses,
        )

    @mock.patch.object(execution.TestExecution, "run")
    async def test_test_could_not_run_returns_failure(
        self, mock_test_run: mock.AsyncMock
    ) -> None:
        """Test that TestCouldNotRun (e.g. missing Merkle hash) causes fx test to exit with code 1."""
        mock_test_run.side_effect = execution.TestCouldNotRun(
            "Missing Merkle hash"
        )
        self._mock_run_command(0)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--no-build", "bar_test"])
        )
        self.assertEqual(ret, 1)

    async def test_count(self) -> None:
        """Test that we can re-run a test multiple times with --count"""

        command_mock = self._mock_run_command(0)

        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        # Run each test 3 times, no parallel to better match behavior of failure case test.
        ret = await main.async_main_wrapper(
            args.parse_args(
                ["--simple", "--no-build", "--count=3", "--parallel=1"]
            )
        )
        self.assertEqual(ret, 0)

        self.assertEqual(
            3,
            sum(["bar_test" in v[0][0] for v in command_mock.call_args_list]),
            command_mock.call_args_list,
        )
        self.assertEqual(
            3,
            sum(["baz_test" in v[0][0] for v in command_mock.call_args_list]),
            command_mock.call_args_list,
        )
        self.assertEqual(
            3,
            sum(
                [
                    "foo-test?hash=" in " ".join(v[0])
                    for v in command_mock.call_args_list
                ]
            ),
            command_mock.call_args_list,
        )

    async def test_count_with_timeout(self) -> None:
        """Test that we abort running the rest of the tests in a --count group if a timeout occurs."""

        command_mock = self._mock_run_command(1)
        command_mock.return_value = command.CommandOutput(
            "", "", 1, 10, None, was_timeout=True
        )

        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        # Run each test 3 times, no parallel to better match behavior of failure case test.
        ret = await main.async_main_wrapper(
            args.parse_args(
                [
                    "--simple",
                    "--no-build",
                    "--count=3",
                    "--parallel=1",
                ]
            )
        )
        self.assertEqual(ret, 1)

        self.assertEqual(
            1,
            sum(["bar_test" in v[0][0] for v in command_mock.call_args_list]),
            command_mock.call_args_list,
        )
        self.assertEqual(
            1,
            sum(["baz_test" in v[0][0] for v in command_mock.call_args_list]),
            command_mock.call_args_list,
        )
        self.assertEqual(
            1,
            sum(
                [
                    "foo-test?hash=" in " ".join(v[0])
                    for v in command_mock.call_args_list
                ]
            ),
            command_mock.call_args_list,
        )

    async def test_no_fail_by_group(self) -> None:
        """Test that we continue running the rest of the tests in a --count group if one fails and --no-fail-by-group is passed."""

        command_mock = self._mock_run_command(1)
        command_mock.return_value = command.CommandOutput(
            "", "", 1, 10, None, was_timeout=True
        )

        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        # Run each test 3 times, no parallel to better match behavior of failure case test.
        ret = await main.async_main_wrapper(
            args.parse_args(
                [
                    "--simple",
                    "--no-build",
                    "--count=3",
                    "--parallel=1",
                    "--no-fail-by-group",
                ]
            )
        )
        self.assertEqual(ret, 1)

        self.assertEqual(
            3,
            sum(["bar_test" in v[0][0] for v in command_mock.call_args_list]),
            command_mock.call_args_list,
        )
        self.assertEqual(
            3,
            sum(["baz_test" in v[0][0] for v in command_mock.call_args_list]),
            command_mock.call_args_list,
        )
        self.assertEqual(
            3,
            sum(
                [
                    "foo-test?hash=" in " ".join(v[0])
                    for v in command_mock.call_args_list
                ]
            ),
            command_mock.call_args_list,
        )

    @parameterized.expand(
        [
            ("existing package server running", True),
            ("no existing package server running", False),
        ]
    )
    async def test_list_command(
        self, _unused_name: str, existing_package_server: bool
    ) -> None:
        """Test that we can list test cases using --list"""

        command_mock = self._mock_run_command(0, stdout="foo::test\nbar::test")

        self._mock_has_package_server_connected_to_device(
            existing_package_server
        )
        self._mock_has_tests_in_base([])

        recorder = event.EventRecorder()

        # This only works if the first test is a device test.
        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--no-build", "--list", "--limit=1"]),
            recorder=recorder,
        )
        self.assertEqual(ret, 0)
        # We expect 1 extra call for the `ffx target list` command to verify/set
        # FUCHSIA_NODENAME, as there is a device test and FUCHSIA_NODENAME is not set.
        if existing_package_server:
            self.assertEqual(command_mock.call_count, 2)
        else:
            # has to run two extra commands: "fx serve" and "fx ffx target repository deregister"
            self.assertEqual(command_mock.call_count, 4)

        events = [
            e.payload.enumerate_test_cases
            async for e in recorder.iter()
            if e.payload is not None
            and e.payload.enumerate_test_cases is not None
        ]
        self.assertEqual(len(events), 1)

        self.assertEqual(
            events[0].test_case_names,
            [
                "foo::test",
                "bar::test",
            ],
        )

        self.assertEqual(
            events[0].command_template,
            main.execution._DEFAULT_TEST_OUTPUT_TEMPLATE,
        )

        call_prefixes = self._make_call_args_prefix_set(
            command_mock.call_args_list
        )

        if existing_package_server:
            self.assertNotIn(
                ("fx", "--dir", self.out_dir, "serve"), call_prefixes
            )
        else:
            self.assertIsSubset(
                {
                    (
                        "fx",
                        "--dir",
                        self.out_dir,
                        "serve",
                        "-l",
                        "0",
                        "--name",
                        "fxtest-temp-test-uuid",
                    )
                },
                call_prefixes,
            )

    async def test_list_failing_command(self) -> None:
        """Test that failing to list test cases using --list results in a nonzero exit code"""

        command_mock = self._mock_run_command(
            100,
            stdout="Failed to create remote control proxy: Timeout attempting to reach target foo.",
        )

        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        recorder = event.EventRecorder()

        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--no-build", "--list", "--limit=1"]),
            recorder=recorder,
        )
        self.assertEqual(ret, 1)
        # We expect 1 extra call for the `ffx target list` command to verify/set
        # FUCHSIA_NODENAME, as there is a device test and FUCHSIA_NODENAME is not set.
        self.assertEqual(command_mock.call_count, 2)

        events = [
            e.payload.enumerate_test_cases
            async for e in recorder.iter()
            if e.payload is not None
            and e.payload.enumerate_test_cases is not None
        ]
        self.assertEqual(len(events), 0)

    async def test_list_mobly_tests(self) -> None:
        """Test that we can list Mobly test cases using --list"""

        mock_enumerate = self._mock_enumerate_mobly_test(
            command.CommandOutput(
                stdout="test_case_1\ntest_case_2\ntest_case_3",
                stderr="",
                return_code=0,
                runtime=10,
                wrapper_return_code=None,
            )
        )

        recorder = event.EventRecorder()

        ret = await main.async_main_wrapper(
            args.parse_args(
                ["--simple", "--no-build", "--list", "host_x64/bar_test"]
            ),
            recorder=recorder,
        )
        self.assertEqual(ret, 0)
        self.assertEqual(mock_enumerate.call_count, 1)

        events = [
            e.payload.enumerate_test_cases
            async for e in recorder.iter()
            if e.payload is not None
            and e.payload.enumerate_test_cases is not None
        ]
        self.assertEqual(len(events), 1)
        self.assertEqual(
            events[0].command_template,
            main.execution._MOBLY_TEST_OUTPUT_TEMPLATE,
        )
        self.assertEqual(
            events[0].test_case_names,
            [
                "test_case_1",
                "test_case_2",
                "test_case_3",
            ],
        )

    async def test_run_mobly_test_with_filter(self) -> None:
        """Test that we can filter Mobly test cases using --test-filter"""

        mock_enumerate = self._mock_enumerate_test_cases(
            command.CommandOutput(
                stdout="test_case_1\ntest_case_2\ntest_case_3",
                stderr="",
                return_code=0,
                runtime=10,
                wrapper_return_code=None,
            )
        )

        command_mock = self._mock_run_command(0)

        # Create a mock test object
        bar_test = test_list_file.Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(
                    name="host_x64/bar_test",
                    label="//src/sys:bar_test(//build/toolchain/host:x64)",
                    path="host_x64/bar_test",
                    os="linux",
                    list_cases_argument="list_mobly_tests",
                )
            )
        )

        # Mock selection to avoid calling dldist
        mock_select = mock.AsyncMock(
            return_value=selection_types.TestSelections(
                selected=[bar_test],
                selected_but_not_run=[],
                best_score={bar_test.name(): 0},
                group_matches=[],
                fuzzy_distance_threshold=3,
            )
        )

        patch = mock.patch("selection.select_tests", mock_select)
        patch.start()
        self.addCleanup(patch.stop)

        recorder = event.EventRecorder()

        ret = await main.async_main_wrapper(
            args.parse_args(
                [
                    "--simple",
                    "--no-build",
                    "--test-filter",
                    "case_2",
                    "host_x64/bar_test",
                ]
            ),
            recorder=recorder,
        )
        self.assertEqual(ret, 0)
        self.assertEqual(mock_enumerate.call_count, 1)

        call_args = command_mock.call_args_list
        found = False
        for call in call_args:
            args_list = list(call[0])
            if "--test_cases" in args_list and "test_case_2" in args_list:
                found = True
                break
        self.assertTrue(
            found,
            f"Did not find --test_cases test_case_2 in calls: {call_args}",
        )

    async def test_list_python_host_tests(self) -> None:
        """Test that we can list python host test cases using --list"""

        mock_enumerate = self._mock_enumerate_test_cases(
            command.CommandOutput(
                stdout="test_case_1\ntest_case_2\ntest_case_3",
                stderr="",
                return_code=0,
                runtime=10,
                wrapper_return_code=None,
            )
        )

        recorder = event.EventRecorder()
        ret = await main.async_main_wrapper(
            args.parse_args(
                ["--simple", "--no-build", "--list", "host_x64/baz_test"]
            ),
            recorder=recorder,
        )
        self.assertEqual(ret, 0)
        self.assertEqual(mock_enumerate.call_count, 1)

        events = [
            e.payload.enumerate_test_cases
            async for e in recorder.iter()
            if e.payload is not None
            and e.payload.enumerate_test_cases is not None
        ]
        self.assertEqual(len(events), 1)
        self.assertEqual(
            events[0].command_template,
            main.execution._PYTHON_HOST_TEST_OUTPUT_TEMPLATE,
        )
        self.assertEqual(
            events[0].test_case_names,
            [
                "test_case_1",
                "test_case_2",
                "test_case_3",
            ],
        )

    @mock.patch("main.run_build", side_effect=[mock.MagicMock(return_code=0)])
    async def test_updateifinbase(self, _build_mock: mock.AsyncMock) -> None:
        """Test that we appropriately update tests in base"""

        command_mock = self._mock_run_command(0)

        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base(["foo-test"])

        ret = await main.async_main_wrapper(
            args.parse_args(
                [
                    "--simple",
                    "--no-build",
                    "--updateifinbase",
                    "--parallel",
                    "1",
                ]
            )
        )
        self.assertEqual(ret, 0)
        call_prefixes = self._make_call_args_prefix_set(
            command_mock.call_args_list
        )
        self.assertIsSubset(
            {
                (
                    "fx",
                    "--dir",
                    self.out_dir,
                    "ota",
                    "--no-build",
                )
            },
            call_prefixes,
        )

    async def test_print_logs_success(self) -> None:
        """Test that print_logs searches for logs, can be given a log,
        and handles invalid data
        """
        env = environment.ExecutionEnvironment.initialize_from_args(
            args.parse_args([])
        )
        assert env.log_file
        # Create a sample log with 3 tests running
        recorder = event.EventRecorder()
        recorder.emit_init()

        # Simulate one test suite
        test_id = recorder.emit_test_suite_started("foo", hermetic=False)
        program_id = recorder.emit_program_start(
            "bar", ["abcd"], parent=test_id
        )
        recorder.emit_program_output(
            program_id, "Data", event.ProgramOutputStream.STDOUT
        )
        recorder.emit_program_termination(program_id, 0)
        recorder.emit_test_suite_ended(
            test_id,
            event.TestSuiteStatus.PASSED,
            message=None,
        )

        test_2 = recorder.emit_test_suite_started(
            "//other:test2", hermetic=True
        )
        test_3 = recorder.emit_test_suite_started(
            "//other:test3", hermetic=True
        )
        program_2 = recorder.emit_program_start(
            "test", ["arg", "1"], parent=test_2
        )
        program_3 = recorder.emit_program_start(
            "test", ["arg", "2"], parent=test_3
        )
        recorder.emit_program_output(
            program_3,
            "line for test 3",
            stream=event.ProgramOutputStream.STDOUT,
        )
        recorder.emit_program_output(
            program_2,
            "line for test 2",
            stream=event.ProgramOutputStream.STDOUT,
        )
        recorder.emit_program_termination(program_2, 0)
        recorder.emit_program_termination(program_3, 0)
        recorder.emit_test_suite_ended(
            test_2, event.TestSuiteStatus.FAILED, message=None
        )
        recorder.emit_test_suite_ended(
            test_3, event.TestSuiteStatus.PASSED, message=None
        )
        recorder.emit_end()

        with gzip.open(env.log_file, "wt") as out_file:
            async for e in recorder.iter():
                json.dump(e.to_dict(), out_file)  # type:ignore[attr-defined]
                print("", file=out_file)

        def assert_print_logs_output(return_code: int, output: str) -> None:
            self.assertEqual(return_code, 0, f"Content was:\n{output}")
            self.assertIsNotNone(
                re.search(r"3 tests were run", output, re.MULTILINE),
                f"Did not find substring, content was:\n{output}",
            )

        # Test finding most recent log file.
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            return_code = main.do_print_logs(args.parse_args([]))
            assert_print_logs_output(return_code, output.getvalue())

        # Test finding specific log file.
        output = io.StringIO()
        new_file_path = os.path.join(env.out_dir, "other-file.json.gz")
        shutil.move(env.log_file, new_file_path)

        with contextlib.redirect_stdout(output):
            return_code = main.do_print_logs(
                args.parse_args(["--logpath", new_file_path])
            )
            self.assertEqual(
                return_code, 0, f"Content was:\n{output.getvalue()}"
            )
            self.assertIsNotNone(
                re.search(r"3 tests were run", output.getvalue(), re.MULTILINE),
                f"Did not find substring, content was:\n{output.getvalue()}",
            )

    async def test_print_logs_failure(self) -> None:
        """Test that --print-logs prints an error and exits if the log cannot be found"""

        # Default search location
        output = io.StringIO()
        with contextlib.redirect_stderr(output):
            self.assertEqual(main.do_print_logs(args.parse_args([])), 1)
            self.assertIsNotNone(
                re.search(r"No log files found", output.getvalue()),
                f"Did not find substring, output was:\n{output.getvalue()}",
            )

        # Specific missing file
        output = io.StringIO()
        with contextlib.redirect_stderr(output):
            with tempfile.TemporaryDirectory() as td:
                path = os.path.join(td, "does-not-exist")
                self.assertEqual(
                    main.do_print_logs(args.parse_args(["--logpath", path])),
                    1,
                )
                self.assertIsNotNone(
                    re.search(r"No log files found", output.getvalue()),
                    f"Did not find substring, output was:\n{output.getvalue()}",
                )

        # Specific file is not a gzip file
        output = io.StringIO()
        with contextlib.redirect_stderr(output):
            with tempfile.TemporaryDirectory() as td:
                path = os.path.join(td, "does-not-exist")
                with open(path, "w") as f:
                    f.writelines(["hello world"])
                self.assertEqual(
                    main.do_print_logs(args.parse_args(["--logpath", path])),
                    1,
                )
                self.assertIsNotNone(
                    re.search(
                        r"File does not appear to be a gzip file",
                        output.getvalue(),
                    ),
                    f"Did not find substring, output was:\n{output.getvalue()}",
                )

    async def test_print_failed_tests(self) -> None:
        """Test that failed-tests prints out the failed tests"""
        env = environment.ExecutionEnvironment.initialize_from_args(
            args.parse_args([])
        )
        assert env.log_file
        # Create a sample log with 2 failed tests and one passing test
        recorder = event.EventRecorder()
        recorder.emit_init()

        # Simulate one test suite
        test_id = recorder.emit_test_suite_started("foo", hermetic=False)
        recorder.emit_test_suite_ended(
            test_id,
            event.TestSuiteStatus.FAILED,
            message=None,
        )

        test_2 = recorder.emit_test_suite_started(
            "//other:test2", hermetic=True
        )
        test_3 = recorder.emit_test_suite_started(
            "//other:test3", hermetic=True
        )

        recorder.emit_test_suite_ended(
            test_2, event.TestSuiteStatus.PASSED, message=None
        )
        recorder.emit_test_suite_ended(
            test_3, event.TestSuiteStatus.FAILED_TO_START, message=None
        )
        recorder.emit_end()

        with gzip.open(env.log_file, "wt") as out_file:
            async for e in recorder.iter():
                json.dump(e.to_dict(), out_file)  # type:ignore[attr-defined]
                print("", file=out_file)

        def assert_print_failed_tests_output(
            return_code: int, output: str
        ) -> None:
            self.assertEqual(return_code, 0, f"Content was:\n{output}")
            self.assertIsNotNone(
                re.search(
                    r"The following tests failed in the previous run:",
                    output,
                    re.MULTILINE,
                ),
                f"Did not find header substring, content was:\n{output}",
            )
            self.assertIsNotNone(
                re.search(r"\* fx test foo", output, re.MULTILINE),
                f"Did not find test 1 substring, content was:\n{output}",
            )
            self.assertIsNotNone(
                re.search(r"\* fx test //other:test3", output, re.MULTILINE),
                f"Did not find test 3 substring, content was:\n{output}",
            )

        # Test finding most recent log file.
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            return_code = main.do_print_failed(args.parse_args([]))
            assert_print_failed_tests_output(return_code, output.getvalue())

    async def test_print_failed_tests_no_failures(self) -> None:
        """Test that failed-tests prints no failed tests"""

        env = environment.ExecutionEnvironment.initialize_from_args(
            args.parse_args([])
        )
        assert env.log_file
        # Create a sample log with 2 failed tests and one passing test
        recorder = event.EventRecorder()
        recorder.emit_init()

        # Simulate one test suite
        test_id = recorder.emit_test_suite_started("foo", hermetic=False)
        recorder.emit_test_suite_ended(
            test_id,
            event.TestSuiteStatus.PASSED,
            message=None,
        )
        recorder.emit_end()

        with gzip.open(env.log_file, "wt") as out_file:
            async for e in recorder.iter():
                json.dump(e.to_dict(), out_file)  # type:ignore[attr-defined]
                print("", file=out_file)

        def assert_print_failed_tests_output(
            return_code: int, output: str
        ) -> None:
            self.assertEqual(return_code, 0, f"Content was:\n{output}")
            self.assertIsNotNone(
                re.search(
                    r"The previous run had no failed tests",
                    output,
                    re.MULTILINE,
                ),
                f"Did not find substring, content was:\n{output}",
            )

        # Test finding most recent log file.
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            return_code = main.do_print_failed(args.parse_args([]))
            assert_print_failed_tests_output(return_code, output.getvalue())

    @mock.patch("main.termout.is_valid", return_value=False)
    async def test_log_to_stdout(self, _termout_mock: mock.Mock) -> None:
        """Test that we can log everything to stdout, and it parses as JSON lines"""

        self._mock_run_command(0)
        self._mock_subprocess_call(0)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            ret = await main.async_main_wrapper(
                args.parse_args(["--logpath", "-"])
            )
            self.assertEqual(ret, 0)
        for line in output.getvalue().splitlines():
            if not line:
                continue
            try:
                json.loads(line)
            except json.JSONDecodeError as e:
                self.fail(
                    f"Failed to parse line as JSON.\nLine: {line}\nError: {e}"
                )

    async def test_artifact_options(self) -> None:
        """Test that we handle artifact output directories and can query their value"""

        self._mock_run_command(0)
        self._mock_subprocess_call(0)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        with self.subTest("no artifact path still produces empty event"):
            with tempfile.TemporaryDirectory() as td:
                logpath = os.path.join(td, "log.json.gz")
                flags = args.parse_args(["--simple", "--logpath", logpath])
                ret = await main.async_main_wrapper(flags)
                self.assertEqual(ret, 0)

                env = environment.ExecutionEnvironment.initialize_from_args(
                    flags, create_log_file=False
                )

                artifact_path: str | None = None
                for log_entry in log.LogSource.from_env(env).read_log():
                    if (event := log_entry.log_event) is not None:
                        if (
                            event.payload is not None
                            and (path := event.payload.artifact_directory_path)
                            is not None
                        ):
                            artifact_path = path
                    self.assertIsNone(log_entry.error)
                    self.assertIsNone(log_entry.warning)
                self.assertEqual(artifact_path, "")

                # Using the output log file, we should get an error requesting the artifact output.
                stderr = io.StringIO()
                with contextlib.redirect_stderr(stderr):
                    ret = main.do_process_previous(
                        args.parse_args(
                            [
                                "-pr",
                                "artifact-path",
                                "--logpath",
                                logpath,
                            ]
                        )
                    )
                    self.assertNotEqual(ret, 0)

                lines = stderr.getvalue().splitlines()
                self.assertEqual(
                    lines,
                    [
                        "ERROR: The previous run did not specify --artifact-output-directory. Run again with that flag set to get the path."
                    ],
                )

        with self.subTest(
            "artifact path by default goes to the top level directory"
        ):
            with tempfile.TemporaryDirectory() as td:
                logpath = os.path.join("log.json.gz")
                artifact_root = os.path.join(td, "artifacts")
                flags = args.parse_args(
                    [
                        "--simple",
                        "--logpath",
                        logpath,
                        "--outdir",
                        artifact_root,
                    ]
                )
                ret = await main.async_main_wrapper(flags)
                self.assertEqual(ret, 0)

                env = environment.ExecutionEnvironment.initialize_from_args(
                    flags, create_log_file=False
                )

                artifact_path = None
                for log_entry in log.LogSource.from_env(env).read_log():
                    if (event := log_entry.log_event) is not None:
                        if (
                            event.payload is not None
                            and (path := event.payload.artifact_directory_path)
                            is not None
                        ):
                            artifact_path = path
                    self.assertIsNone(log_entry.error)
                    self.assertIsNone(log_entry.warning)
                self.assertEqual(artifact_path, artifact_root)

                # Path gets created automatically.
                self.assertTrue(os.path.isdir(artifact_root))

                # Delete the artifact directory, checking what happens when it does not exist.
                shutil.rmtree(artifact_root)

                # Using the output log file, we should see an error getting the path because the directory will not be present.
                stderr = io.StringIO()
                with contextlib.redirect_stderr(stderr):
                    ret = main.do_process_previous(
                        args.parse_args(
                            [
                                "-pr",
                                "artifact-path",
                                "--logpath",
                                logpath,
                            ]
                        )
                    )
                    self.assertNotEqual(ret, 0)

                lines = stderr.getvalue().splitlines()
                self.assertEqual(
                    lines,
                    [
                        "ERROR: The artifact directory is missing, it may have been deleted."
                    ],
                )

                # Create the artifact directory. Listing artifact path should work now.
                os.makedirs(artifact_root)
                stdout = mock.MagicMock(wraps=io.StringIO())
                stdout.fileno = lambda: -1
                with contextlib.redirect_stdout(stdout):
                    ret = main.do_process_previous(
                        args.parse_args(
                            [
                                "-pr",
                                "artifact-path",
                                "--logpath",
                                logpath,
                            ]
                        )
                    )
                    self.assertEqual(ret, 0)

                lines = stdout.getvalue().splitlines()
                self.assertEqual(
                    lines,
                    [artifact_root],
                )

        with self.subTest(
            "--timestamp-artifacts causes artifacts to go to subdir"
        ):
            with tempfile.TemporaryDirectory() as td:
                logpath = os.path.join("log.json.gz")
                artifact_root = os.path.join(td, "artifacts")
                flags = args.parse_args(
                    [
                        "--simple",
                        "--logpath",
                        logpath,
                        "--outdir",
                        artifact_root,
                        "--timestamp-artifacts",
                    ]
                )
                ret = await main.async_main_wrapper(flags)
                self.assertEqual(ret, 0)

                env = environment.ExecutionEnvironment.initialize_from_args(
                    flags, create_log_file=False
                )

                artifact_path = None
                for log_entry in log.LogSource.from_env(env).read_log():
                    if (event := log_entry.log_event) is not None:
                        if (
                            event.payload is not None
                            and (path := event.payload.artifact_directory_path)
                            is not None
                        ):
                            artifact_path = path
                    self.assertIsNone(log_entry.error)
                    self.assertIsNone(log_entry.warning)
                self.assertNotEqual(artifact_path, artifact_root)
                assert artifact_path is not None
                self.assertEqual(
                    os.path.commonprefix([artifact_path, artifact_root]),
                    artifact_root,
                )

        with self.subTest(
            "it is an error to output to an existing, non-empty directory"
        ):
            with tempfile.TemporaryDirectory() as td:
                logpath = os.path.join("log.json.gz")
                artifact_root = os.path.join(td, "artifacts")
                os.mkdir(artifact_root)
                with open(os.path.join(artifact_root, "some_file"), "w") as f:
                    f.write("Demo data")
                flags = args.parse_args(
                    [
                        "--simple",
                        "--logpath",
                        logpath,
                        "--outdir",
                        artifact_root,
                        "--no-timestamp-artifacts",
                    ]
                )
                ret = await main.async_main_wrapper(flags)
                self.assertEqual(ret, 1)

                env = environment.ExecutionEnvironment.initialize_from_args(
                    flags, create_log_file=False
                )

                artifact_path = None
                found_error = False
                for log_entry in log.LogSource.from_env(env).read_log():
                    if (event := log_entry.log_event) is not None:
                        if (error_message := event.error) is not None:
                            if (
                                "Your output directory already exists"
                                in error_message
                            ):
                                found_error = True
                                break

                self.assertTrue(
                    found_error,
                    "Expected to find an error about output directory existing",
                )

    async def test_previous_stats_analytics(self) -> None:
        """Test that --previous stats-analytics outputs valid compact JSON."""
        self._mock_run_command(0)
        self._mock_subprocess_call(0)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        with tempfile.TemporaryDirectory() as td:
            logpath = os.path.join(td, "log.json.gz")
            flags = args.parse_args(["--simple", "--logpath", logpath])
            ret = await main.async_main_wrapper(flags)
            self.assertEqual(ret, 0)

            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                ret = main.do_process_previous(
                    args.parse_args(
                        [
                            "-pr",
                            "stats-analytics",
                            "--logpath",
                            logpath,
                        ]
                    )
                )
                self.assertEqual(ret, 0)

            output_str = stdout.getvalue().strip()
            payload = json.loads(output_str)
            self.assertIn("t", payload)
            self.assertIn("a", payload)
            self.assertIn("ab", payload)
            self.assertIn("top", payload)
            self.assertIn("sum", payload)
            self.assertIn("sel", payload)
            self.assertEqual(payload["sel"], "")
            self.assertEqual(payload["a"], 0)
            self.assertIsNone(payload["ab"])
            self.assertIsInstance(payload["top"], list)
            self.assertIsInstance(payload["sum"], dict)
            self.assertNotIn(": ", output_str)
            self.assertNotIn(", ", output_str)

    async def test_previous_stats_analytics_with_selection(self) -> None:
        """Test that --previous stats-analytics outputs canonical selection."""
        self._mock_run_command(0)
        self._mock_subprocess_call(0)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        bar_test = test_list_file.Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(
                    name="host_x64/bar_test",
                    label="//src/sys:bar_test(//build/toolchain/host:x64)",
                    path="host_x64/bar_test",
                    os="linux",
                )
            )
        )
        mock_select = mock.AsyncMock(
            return_value=selection_types.TestSelections(
                selected=[bar_test],
                selected_but_not_run=[],
                best_score={bar_test.name(): 0},
                group_matches=[],
                fuzzy_distance_threshold=3,
            )
        )
        patch = mock.patch("selection.select_tests", mock_select)
        patch.start()
        self.addCleanup(patch.stop)

        with tempfile.TemporaryDirectory() as td:
            logpath = os.path.join(td, "log.json.gz")
            flags = args.parse_args(
                ["--simple", "bar_test", "--logpath", logpath]
            )
            ret = await main.async_main_wrapper(flags)
            self.assertEqual(ret, 0)

            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                ret = main.do_process_previous(
                    args.parse_args(
                        [
                            "-pr",
                            "stats-analytics",
                            "--logpath",
                            logpath,
                        ]
                    )
                )
                self.assertEqual(ret, 0)

            output_str = stdout.getvalue().strip()
            payload = json.loads(output_str)
            self.assertIn("sel", payload)
            self.assertEqual(payload["sel"], "bar_test")

    async def test_save_log_path_to_file(self) -> None:
        """Tests that --save-log-path-to-file records the log path."""
        self._mock_run_command(0)
        self._mock_subprocess_call(0)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        bar_test = test_list_file.Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(
                    name="host_x64/bar_test",
                    label="//src/sys:bar_test(//build/toolchain/host:x64)",
                    path="host_x64/bar_test",
                    os="linux",
                )
            )
        )
        mock_select = mock.AsyncMock(
            return_value=selection_types.TestSelections(
                selected=[bar_test],
                selected_but_not_run=[],
                best_score={bar_test.name(): 0},
                group_matches=[],
                fuzzy_distance_threshold=3,
            )
        )
        patch = mock.patch("selection.select_tests", mock_select)
        patch.start()
        self.addCleanup(patch.stop)

        with tempfile.TemporaryDirectory() as td:
            logpath = os.path.join(td, "custom.log.json.gz")
            ptr_file = os.path.join(td, "logpath.ptr")
            flags = args.parse_args(
                [
                    "--simple",
                    "bar_test",
                    "--logpath",
                    logpath,
                    "--save-log-path-to-file",
                    ptr_file,
                ]
            )
            ret = await main.async_main_wrapper(flags)
            self.assertEqual(ret, 0)
            self.assertTrue(os.path.exists(ptr_file))
            with open(ptr_file) as f:
                saved_path = f.read().strip()
            self.assertEqual(saved_path, os.path.abspath(logpath))

    async def test_save_log_path_to_file_no_log(self) -> None:
        """Tests that --no-log does not populate --save-log-path-to-file."""
        self._mock_run_command(0)
        self._mock_subprocess_call(0)
        self._mock_has_package_server_connected_to_device(True)
        self._mock_has_tests_in_base([])

        bar_test = test_list_file.Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(
                    name="host_x64/bar_test",
                    label="//src/sys:bar_test(//build/toolchain/host:x64)",
                    path="host_x64/bar_test",
                    os="linux",
                )
            )
        )
        mock_select = mock.AsyncMock(
            return_value=selection_types.TestSelections(
                selected=[bar_test],
                selected_but_not_run=[],
                best_score={bar_test.name(): 0},
                group_matches=[],
                fuzzy_distance_threshold=3,
            )
        )
        patch = mock.patch("selection.select_tests", mock_select)
        patch.start()
        self.addCleanup(patch.stop)

        with tempfile.TemporaryDirectory() as td:
            ptr_file = os.path.join(td, "logpath.ptr")
            flags = args.parse_args(
                [
                    "--simple",
                    "bar_test",
                    "--no-log",
                    "--save-log-path-to-file",
                    ptr_file,
                ]
            )
            ret = await main.async_main_wrapper(flags)
            self.assertEqual(ret, 0)
            self.assertFalse(os.path.exists(ptr_file))

    async def test_list_runtime_deps_success(self) -> None:
        """Tests the successful listing of runtime dependencies."""
        deps_path = "path/to/my_deps.json"
        full_deps_path = os.path.join(self.out_dir, deps_path)
        os.makedirs(os.path.dirname(full_deps_path), exist_ok=True)
        with open(full_deps_path, "w") as f:
            json.dump(["dep1", "dep2"], f)

        empty_deps_path = "path/to/empty_deps.json"
        full_empty_deps_path = os.path.join(self.out_dir, empty_deps_path)
        os.makedirs(os.path.dirname(full_empty_deps_path), exist_ok=True)
        with open(full_empty_deps_path, "w") as f:
            json.dump([], f)

        mock_test_with_deps = mock.MagicMock()
        mock_test_with_deps.name.return_value = "test_with_deps"
        mock_test_with_deps.build.test.runtime_deps = deps_path

        mock_test_with_empty_deps = mock.MagicMock()
        mock_test_with_empty_deps.name.return_value = "test_with_empty_deps"
        mock_test_with_empty_deps.build.test.runtime_deps = empty_deps_path

        mock_test_without_deps = mock.MagicMock()
        mock_test_without_deps.name.return_value = "test_without_deps"
        mock_test_without_deps.build.test.runtime_deps = None

        mock_selections = mock.MagicMock()
        mock_selections.selected = [
            mock_test_with_deps,
            mock_test_with_empty_deps,
            mock_test_without_deps,
        ]
        mock_selections.selected_but_not_run = []

        selection_patch = mock.patch(
            "main.selection.select_tests",
            mock.AsyncMock(return_value=mock_selections),
        )
        selection_patch.start()
        self.addCleanup(selection_patch.stop)

        validate_patch = mock.patch(
            "main.AsyncMain._validate_test_selections",
            mock.AsyncMock(return_value=None),
        )
        validate_patch.start()
        self.addCleanup(validate_patch.stop)

        recorder = event.EventRecorder()
        ret = await main.async_main_wrapper(
            args.parse_args(["--simple", "--list-runtime-deps", "--no-build"]),
            recorder=recorder,
        )
        self.assertEqual(ret, 0)
        payloads = [
            e.payload.user_message.value
            async for e in recorder.iter()
            if e.payload
            and e.payload.user_message
            and e.payload.user_message.value
        ]
        start_index = payloads.index("test_with_deps:")
        self.assertNotEqual(start_index, -1)
        deps_full_output = payloads[start_index:]
        expected_output = [
            "test_with_deps:",
            f"  Runtime deps file at: {full_deps_path}",
            "  dep1",
            "  dep2",
            "test_with_empty_deps:",
            f"  Runtime deps file at: {full_empty_deps_path}",
            "  File is empty",
            "test_without_deps:",
            "  No runtime deps found for this test",
        ]
        self.assertListEqual(deps_full_output, expected_output)

    async def test_list_runtime_deps_file_not_found(self) -> None:
        """Tests that a missing runtime_deps file raises an exception."""
        missing_deps_path = "path/to/non_existent_deps.json"
        mock_test = mock.MagicMock()
        mock_test.name.return_value = "test_with_missing_deps"
        mock_test.build.test.runtime_deps = missing_deps_path

        mock_selections = mock.MagicMock()
        mock_selections.selected = [mock_test]
        mock_selections.selected_but_not_run = []

        selection_patch = mock.patch(
            "main.selection.select_tests",
            mock.AsyncMock(return_value=mock_selections),
        )
        selection_patch.start()
        self.addCleanup(selection_patch.stop)

        validate_patch = mock.patch(
            "main.AsyncMain._validate_test_selections",
            mock.AsyncMock(return_value=None),
        )
        validate_patch.start()
        self.addCleanup(validate_patch.stop)

        with self.assertRaises(FileNotFoundError):
            await main.async_main_wrapper(
                args.parse_args(
                    ["--simple", "--list-runtime-deps", "--no-build"]
                )
            )

    async def test_boot_test_selection_validation_error(self) -> None:
        """Test that selecting a boot test raises _SelectionValidationError."""
        mock_test = mock.MagicMock()
        mock_test.name.return_value = "my_boot_test"
        mock_test.is_boot_test.return_value = True

        mock_selections = mock.MagicMock()
        mock_selections.group_matches = []
        mock_selections.selected = [mock_test]

        app = main.AsyncMain.__new__(main.AsyncMain)
        app._recorder = mock.Mock()
        app._flags = mock.Mock()
        app._exec_env = mock.Mock()

        with self.assertRaises(main.AsyncMain._SelectionValidationError) as ctx:
            await app._validate_test_selections(mock_selections)

        err_msg = str(ctx.exception)
        self.assertIn("Boot tests are not supported by `fx test`.", err_msg)
        self.assertIn("Use `fx run-boot-test` or `fx core-tests`", err_msg)
        self.assertIn("my_boot_test", err_msg)

    @mock.patch("main.run_commands_in_parallel")
    async def test_validate_test_selections_missing_groups_raises_even_if_suggestions_fail(
        self, mock_run_commands: mock.AsyncMock
    ) -> None:
        """Tests that _validate_test_selections raises even if suggestions return None."""
        mock_run_commands.return_value = [None]
        mock_group = mock.MagicMock()
        mock_group.names = {"non_existent_test"}
        mock_group.components = set()
        mock_group.packages = set()
        mock_group.__str__ = mock.MagicMock(return_value="non_existent_test")

        mock_selections = mock.MagicMock()
        mock_selections.group_matches = [(mock_group, [])]
        mock_selections.selected = []

        app = main.AsyncMain.__new__(main.AsyncMain)
        app._recorder = mock.Mock()
        app._flags = mock.Mock()
        app._flags.exact = False
        app._flags.show_suggestions = True
        app._flags.suggestion_count = 5
        app._flags.style = False
        app._flags.host = False
        app._flags.device = False
        app._flags.remote_suggestions = False
        app._flags.remote_suggestion_builder = []
        app._flags.selection = ["non_existent_test"]
        app._exec_env = mock.Mock()
        app._exec_env.fx_cmd_line.side_effect = lambda *args: list(args)

        with self.assertRaises(main.AsyncMain._SelectionValidationError) as ctx:
            await app._validate_test_selections(mock_selections)

        self.assertIn(
            "No tests found for the following selections", str(ctx.exception)
        )

    async def test_validate_test_selections_partial_match_allowed_by_default(
        self,
    ) -> None:
        """Tests that a partial match with some empty selection groups is allowed by default."""
        mock_test = mock.MagicMock()
        mock_test.name.return_value = "my_test"
        mock_test.is_boot_test.return_value = False
        mock_test.is_e2e_test.return_value = False

        mock_group1 = mock.MagicMock()
        mock_group1.names = {"my_test"}
        mock_group1.components = set()
        mock_group1.packages = set()

        mock_group2 = mock.MagicMock()
        mock_group2.names = {"missing_test"}
        mock_group2.components = set()
        mock_group2.packages = set()
        mock_group2.__str__ = mock.MagicMock(return_value="missing_test")

        mock_selections = mock.MagicMock()
        mock_selections.group_matches = [
            (mock_group1, [mock_test]),
            (mock_group2, []),
        ]
        mock_selections.selected = [mock_test]

        app = main.AsyncMain.__new__(main.AsyncMain)
        app._recorder = mock.Mock()
        app._flags = mock.Mock()
        app._flags.exact = False
        app._flags.show_suggestions = False
        app._flags.allow_empty_selection = True
        app._flags.selection = ["my_test", "missing_test"]
        app._flags.e2e = False
        app._exec_env = mock.Mock()

        # Should not raise exception
        await app._validate_test_selections(mock_selections)

    async def test_validate_test_selections_partial_match_fails_when_not_allowed(
        self,
    ) -> None:
        """Tests that a partial match fails when allow_empty_selection is False."""
        mock_test = mock.MagicMock()
        mock_test.name.return_value = "my_test"
        mock_test.is_boot_test.return_value = False
        mock_test.is_e2e_test.return_value = False

        mock_group1 = mock.MagicMock()
        mock_group1.names = {"my_test"}
        mock_group1.components = set()
        mock_group1.packages = set()

        mock_group2 = mock.MagicMock()
        mock_group2.names = {"missing_test"}
        mock_group2.components = set()
        mock_group2.packages = set()
        mock_group2.__str__ = mock.MagicMock(return_value="missing_test")

        mock_selections = mock.MagicMock()
        mock_selections.group_matches = [
            (mock_group1, [mock_test]),
            (mock_group2, []),
        ]
        mock_selections.selected = [mock_test]

        app = main.AsyncMain.__new__(main.AsyncMain)
        app._recorder = mock.Mock()
        app._flags = mock.Mock()
        app._flags.exact = False
        app._flags.show_suggestions = False
        app._flags.allow_empty_selection = False
        app._flags.selection = ["my_test", "missing_test"]
        app._flags.e2e = False
        app._exec_env = mock.Mock()

        with self.assertRaises(main.AsyncMain._SelectionValidationError) as ctx:
            await app._validate_test_selections(mock_selections)

        self.assertIn("missing_test", str(ctx.exception))

    async def test_validate_test_selections_no_matches_with_selection(
        self,
    ) -> None:
        """Tests that _validate_test_selections raises when selection flag is given but selected is empty."""
        mock_selections = mock.MagicMock()
        mock_selections.group_matches = []
        mock_selections.selected = []

        app = main.AsyncMain.__new__(main.AsyncMain)
        app._recorder = mock.Mock()
        app._flags = mock.Mock()
        app._flags.exact = False
        app._flags.show_suggestions = False
        app._flags.selection = ["some_test"]
        app._exec_env = mock.Mock()

        with self.assertRaises(main.AsyncMain._SelectionValidationError) as ctx:
            await app._validate_test_selections(mock_selections)

        self.assertIn("No tests found matching criteria.", str(ctx.exception))

    async def test_has_active_device(self) -> None:
        """Tests that _has_active_device correctly detects active devices."""
        app = main.AsyncMain.__new__(main.AsyncMain)
        app._has_active_device = (
            TestMainIntegration.ORIGINAL_HAS_ACTIVE_DEVICE.__get__(
                app, main.AsyncMain
            )
        )
        app._recorder = mock.Mock()

        exec_env = mock.Mock()
        exec_env.fx_cmd_line.side_effect = lambda *args: list(args)
        app._exec_env = exec_env

        mock_run_command = mock.AsyncMock()
        patch = mock.patch.object(execution, "run_command", mock_run_command)
        patch.start()
        self.addCleanup(patch.stop)

        # Case 1: Specific/default target is reachable.
        mock_run_command.side_effect = [
            mock.Mock(return_code=0, stdout="default-target\n"),
            mock.Mock(return_code=0, stdout=""),
        ]
        with mock.patch.dict(os.environ, {}, clear=True):
            self.assertTrue(await app._has_active_device())
        self.assertEqual(mock_run_command.call_count, 2)
        mock_run_command.assert_any_call(
            "ffx",
            "target",
            "default",
            "get",
            recorder=app._recorder,
            quiet_mode=True,
            timeout=30.0,
        )
        mock_run_command.assert_any_call(
            "ffx",
            "-t",
            "default-target",
            "target",
            "echo",
            recorder=app._recorder,
            quiet_mode=True,
            timeout=30.0,
        )

        # Case 2: Specific target is unreachable, fallback succeeds with active device.
        mock_run_command.reset_mock()
        mock_run_command.side_effect = [
            mock.Mock(return_code=0, stdout="default-target\n"),  # default get
            mock.Mock(return_code=1, stdout=""),  # echo fails
            mock.Mock(
                return_code=0,
                stdout=json.dumps(
                    [
                        {"nodename": "device1", "rcs_state": "N"},
                        {"nodename": "device2", "rcs_state": "Y"},
                    ]
                ),
            ),  # list
        ]
        with mock.patch.dict(os.environ, {}, clear=True):
            self.assertTrue(await app._has_active_device())

        # Case 3: Fallback has only inactive/unreachable devices.
        mock_run_command.reset_mock()
        mock_run_command.side_effect = [
            mock.Mock(return_code=0, stdout="default-target\n"),  # default get
            mock.Mock(return_code=1, stdout=""),  # echo fails
            mock.Mock(
                return_code=0,
                stdout=json.dumps(
                    [
                        {"nodename": "device1", "rcs_state": "N"},
                        {"nodename": "device2", "rcs_state": "N"},
                    ]
                ),
            ),  # list
        ]
        with mock.patch.dict(os.environ, {}, clear=True):
            self.assertFalse(await app._has_active_device())

        # Case 4: Fallback returns empty list.
        mock_run_command.reset_mock()
        mock_run_command.side_effect = [
            mock.Mock(return_code=0, stdout="default-target\n"),  # default get
            mock.Mock(return_code=1, stdout=""),  # echo fails
            mock.Mock(return_code=0, stdout="[]"),  # list empty
        ]
        with mock.patch.dict(os.environ, {}, clear=True):
            self.assertFalse(await app._has_active_device())

        # Case 5: FUCHSIA_NODENAME is set and echo succeeds.
        mock_run_command.reset_mock()
        mock_run_command.side_effect = [
            mock.Mock(return_code=0, stdout=""),  # echo succeeds
        ]
        with mock.patch.dict(os.environ, {"FUCHSIA_NODENAME": "env-target"}):
            self.assertTrue(await app._has_active_device())
        mock_run_command.assert_called_once_with(
            "ffx",
            "-t",
            "env-target",
            "target",
            "echo",
            recorder=app._recorder,
            quiet_mode=True,
            timeout=30.0,
        )

    @mock.patch("execution.run_command")
    async def test_run_build(self, mock_run_command: mock.AsyncMock) -> None:
        """Test run_build executes asynchronously with expected args."""
        mock_run_command.return_value = mock.MagicMock(return_code=0)
        exec_env = environment.ExecutionEnvironment.initialize_from_args(
            args.parse_args(["--simple"])
        )
        recorder = event.EventRecorder()
        parent_id = event.Id(1)
        abort_signal = asyncio.Event()

        res = await main.run_build(
            exec_env,
            ["//src:foo"],
            recorder=recorder,
            parent=parent_id,
            abort_signal=abort_signal,
        )
        self.assertIsNotNone(res)
        self.assertEqual(res.return_code, 0)
        mock_run_command.assert_called_once_with(
            *exec_env.fx_cmd_line("build", "//src:foo"),
            recorder=recorder,
            parent=parent_id,
            abort_signal=abort_signal,
            print_verbatim=False,
            quiet_mode=True,
        )

    @mock.patch("main.run_build")
    async def test_do_build_passes_build_id(
        self, mock_build: mock.AsyncMock
    ) -> None:
        """Test that _do_build passes build_id as parent to run_build."""
        mock_build.return_value = mock.MagicMock(return_code=0)

        mock_test = test_list_file.Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(
                    name="my_host_test",
                    label="//src:my_host_test(//build/toolchain:host_x64)",
                    os="linux",
                )
            )
        )
        selections = selection_types.TestSelections(
            selected=[mock_test],
            selected_but_not_run=[],
            best_score={},
            group_matches=[],
            fuzzy_distance_threshold=0,
        )

        app = main.AsyncMain.__new__(main.AsyncMain)
        app._recorder = event.EventRecorder()
        app._flags = args.parse_args(["--simple"])
        app._exec_env = environment.ExecutionEnvironment.initialize_from_args(
            app._flags
        )
        app._end_execution_request_event = asyncio.Event()
        self.assertTrue(await app._do_build(selections))
        mock_build.assert_called_once()
        self.assertIsNotNone(mock_build.call_args.kwargs.get("parent"))

    @mock.patch("main.run_build")
    async def test_do_build_failure_outputs_compiler_error(
        self, mock_build: mock.AsyncMock
    ) -> None:
        """Test that build failure outputs compiler diagnostics to the event recorder."""
        mock_build.return_value = mock.MagicMock(
            return_code=1,
            stdout="",
            stderr="ninja: error: build.ninja:1: syntax error",
        )

        app = main.AsyncMain.__new__(main.AsyncMain)
        recorder = mock.MagicMock()
        app._recorder = recorder
        app._flags = args.parse_args(["--simple"])
        app._exec_env = environment.ExecutionEnvironment.initialize_from_args(
            app._flags
        )
        app._end_execution_request_event = asyncio.Event()

        mock_test = test_list_file.Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(
                    name="my_host_test",
                    label="//src:my_host_test(//build/toolchain:host_x64)",
                    os="linux",
                )
            )
        )

        selections = selection_types.TestSelections(
            selected=[mock_test],
            selected_but_not_run=[],
            best_score={},
            group_matches=[],
            fuzzy_distance_threshold=0,
        )

        res = await app._do_build(selections)
        self.assertFalse(res)
        recorder.emit_verbatim_message.assert_called_with(
            "ninja: error: build.ninja:1: syntax error"
        )
        recorder.emit_end.assert_called_with(
            "Build returned non-zero exit code 1", id=mock.ANY
        )

    @mock.patch("main.run_build")
    async def test_do_build_failure_outputs_stdout_and_stderr(
        self, mock_build: mock.AsyncMock
    ) -> None:
        """Test that build failure concatenates compiler stdout and stderr diagnostics."""
        mock_build.return_value = mock.MagicMock(
            return_code=1,
            stdout="../../src/foo.cc:1: error: expected ';'",
            stderr="Hint: run `fx build` with --log",
        )

        app = main.AsyncMain.__new__(main.AsyncMain)
        recorder = mock.MagicMock()
        app._recorder = recorder
        app._flags = args.parse_args(["--simple"])
        app._exec_env = environment.ExecutionEnvironment.initialize_from_args(
            app._flags
        )
        app._end_execution_request_event = asyncio.Event()

        mock_test = test_list_file.Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(
                    name="my_host_test",
                    label="//src:my_host_test(//build/toolchain:host_x64)",
                    os="linux",
                )
            )
        )

        selections = selection_types.TestSelections(
            selected=[mock_test],
            selected_but_not_run=[],
            best_score={},
            group_matches=[],
            fuzzy_distance_threshold=0,
        )

        res = await app._do_build(selections)
        self.assertFalse(res)
        recorder.emit_verbatim_message.assert_called_with(
            "../../src/foo.cc:1: error: expected ';'\nHint: run `fx build` with --log"
        )
        recorder.emit_end.assert_called_with(
            "Build returned non-zero exit code 1", id=mock.ANY
        )

    @mock.patch("main.run_build")
    async def test_do_build_aborted(self, mock_build: mock.AsyncMock) -> None:
        """Test that _do_build handles build abort (None return value)."""
        mock_build.return_value = None

        app = main.AsyncMain.__new__(main.AsyncMain)
        recorder = mock.MagicMock()
        app._recorder = recorder
        app._flags = args.parse_args(["--simple"])
        app._exec_env = environment.ExecutionEnvironment.initialize_from_args(
            app._flags
        )
        app._end_execution_request_event = asyncio.Event()

        mock_test = test_list_file.Test(
            build=tests_json_file.TestEntry(
                test=tests_json_file.TestSection(
                    name="my_host_test",
                    label="//src:my_host_test(//build/toolchain:host_x64)",
                    os="linux",
                )
            )
        )

        selections = selection_types.TestSelections(
            selected=[mock_test],
            selected_but_not_run=[],
            best_score={},
            group_matches=[],
            fuzzy_distance_threshold=0,
        )

        res = await app._do_build(selections)
        self.assertFalse(res)
        recorder.emit_end.assert_called_with(
            "Build returned non-zero exit code -1", id=mock.ANY
        )

    @mock.patch("sys.argv", ["fx_test", "my_device_test"])
    @mock.patch("agents.agents.is_invoked_by_agent", return_value=True)
    async def test_agent_debugging_mode_hint_on_failure(
        self, _mock_is_agent: mock.MagicMock
    ) -> None:
        """Test that test failure emits --agent-debugging-mode into summary hints for agents."""
        app = main.AsyncMain.__new__(main.AsyncMain)
        recorder = mock.MagicMock()
        app._recorder = recorder
        app._summary_val = summary.RunSummary()
        app._flags = args.parse_args(["--agent-output"])
        app._flags.validate()
        app._teardown_resources = mock.AsyncMock()

        # Simulate the failure path in _main_impl
        flags = app._flags
        if not flags.debugger_will_attach() and not flags.host:
            msg = "To debug with zxdb: fx test --break-on-failure {}".format(
                " ".join(["my_device_test"])
            )
            if agents_lib.is_invoked_by_agent():
                msg = "To debug with fx debug cli: fx test --agent-debugging-mode {}".format(
                    " ".join(["my_device_test"])
                )

            app._summary.hints.append(msg)
            recorder.emit_instruction_message(msg)

        self.assertIn(
            "To debug with fx debug cli: fx test --agent-debugging-mode my_device_test",
            app._summary.hints,
        )
        recorder.emit_instruction_message.assert_called_with(
            "To debug with fx debug cli: fx test --agent-debugging-mode my_device_test"
        )
        md = app._summary.to_markdown()
        self.assertIn("## Hints", md)
        self.assertIn(
            "- To debug with fx debug cli: fx test --agent-debugging-mode my_device_test",
            md,
        )

    async def test_has_package_server_connected_to_device_called_multiple_times(
        self,
    ) -> None:
        """Calling has_package_server_connected_to_device multiple times does not reuse awaited coroutine."""
        exec_env = mock.MagicMock()
        exec_env.fx_cmd_line.side_effect = lambda *args: ["fx"] + list(args)
        recorder = mock.MagicMock()

        with mock.patch("execution.run_command") as mock_run:
            mock_run.return_value = mock.MagicMock(
                return_code=0,
                stdout=json.dumps(
                    {
                        "ok": {
                            "data": [
                                {
                                    "name": "devhost",
                                    "aliases": ["fuchsia.com"],
                                }
                            ]
                        }
                    }
                ),
            )
            res1 = await main.has_package_server_connected_to_device(
                exec_env, recorder
            )
            res2 = await main.has_package_server_connected_to_device(
                exec_env, recorder
            )
            self.assertTrue(res1)
            self.assertTrue(res2)
            self.assertEqual(mock_run.call_count, 4)

    async def test_has_package_server_connected_to_device_host_not_running(
        self,
    ) -> None:
        """has_package_server_connected_to_device returns False when host server is not running."""
        exec_env = mock.MagicMock()
        exec_env.fx_cmd_line.side_effect = lambda *args: ["fx"] + list(args)
        recorder = mock.MagicMock()

        with mock.patch("execution.run_command") as mock_run:
            mock_run.return_value = mock.MagicMock(return_code=1, stdout="")
            res = await main.has_package_server_connected_to_device(
                exec_env, recorder
            )
            self.assertFalse(res)
            self.assertEqual(mock_run.call_count, 1)

    async def test_has_package_server_connected_to_device_not_registered_on_target(
        self,
    ) -> None:
        """has_package_server_connected_to_device returns False when host server is running but not registered on target."""
        exec_env = mock.MagicMock()
        exec_env.fx_cmd_line.side_effect = lambda *args: ["fx"] + list(args)
        recorder = mock.MagicMock()

        with mock.patch("execution.run_command") as mock_run:
            mock_run.side_effect = [
                mock.MagicMock(return_code=0, stdout=""),
                mock.MagicMock(
                    return_code=0,
                    stdout=json.dumps({"ok": {"data": []}}),
                ),
            ]
            res = await main.has_package_server_connected_to_device(
                exec_env, recorder
            )
            self.assertFalse(res)
            self.assertEqual(mock_run.call_count, 2)

    async def test_post_build_checklist_corrupt_base_packages(self) -> None:
        """Corrupted base_packages.list does not crash _post_build_checklist."""
        app = main.AsyncMain.__new__(main.AsyncMain)
        recorder = mock.MagicMock()
        app._recorder = recorder
        exec_env = mock.MagicMock()
        with tempfile.TemporaryDirectory() as td:
            base_file = os.path.join(td, "base_packages.list")
            with open(base_file, "w") as f:
                f.write("NOT VALID JSON")
            exec_env.out_dir = td
            app._exec_env = exec_env

            tests = mock.MagicMock()
            tests.has_device_test.return_value = True
            # Should return False safely without raising JSONDecodeError
            res = await app._post_build_checklist(tests, event.Id(1))
            self.assertFalse(res)

    def test_is_fuchsia_repo_registered(self) -> None:
        """_is_fuchsia_repo_registered correctly identifies repository registration."""
        # JSON with aliases containing fuchsia.com
        data_aliases = json.dumps(
            {"ok": {"data": [{"name": "devhost", "aliases": ["fuchsia.com"]}]}}
        )
        self.assertTrue(main._is_fuchsia_repo_registered(data_aliases))

        # JSON with name equal to fuchsia.com
        data_name = json.dumps(
            {"ok": {"data": [{"name": "fuchsia.com", "aliases": []}]}}
        )
        self.assertTrue(main._is_fuchsia_repo_registered(data_name))

        # JSON with name equal to fuchsia-pkg://fuchsia.com
        data_pkg = json.dumps(
            {
                "ok": {
                    "data": [
                        {"name": "fuchsia-pkg://fuchsia.com", "aliases": []}
                    ]
                }
            }
        )
        self.assertTrue(main._is_fuchsia_repo_registered(data_pkg))

        # JSON empty data
        self.assertFalse(
            main._is_fuchsia_repo_registered(json.dumps({"ok": {"data": []}}))
        )

        # Invalid JSON raises JSONDecodeError
        with self.assertRaises(json.JSONDecodeError):
            main._is_fuchsia_repo_registered("not json")
