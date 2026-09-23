# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import argparse
import asyncio
import atexit
from collections import defaultdict
from dataclasses import dataclass
from dataclasses import field
import enum
import json
import os
import re
import subprocess
import sys
import tempfile
import textwrap
import time
import typing
from typing import List
import uuid

import agents.agents as agents_lib
import async_utils.command as command
import async_utils.signals as signals
import statusinfo
import termout

import args
import config
import console
import dataparse
import debugger
import environment
import event
import execution
import find_affected
import log
import package_repository
import selection
import selection_types
import summary
import test_list_file
import tests_json_file

# Subprocess and probe timeout constants in seconds.
_DEFAULT_PROBE_TIMEOUT_SECONDS: float = 30.0
_PACKAGE_SERVER_PROBE_TIMEOUT_SECONDS: float = 15.0
_DEFAULT_EMU_START_TIMEOUT_SECONDS: float = 60.0
_EMU_START_TIMEOUT_PADDING_SECONDS: float = 15.0
_TARGET_WAIT_TIMEOUT_SECONDS: float = 30.0


def main() -> None:
    # Main entrypoint.
    # Set up the event loop to catch termination signals (i.e. Ctrl+C), and
    # cancel the main task when they are received.
    try:
        config_file = config.load_config()
    except argparse.ArgumentError as e:
        print(f"Failed to parse config: {e.message}")
        sys.exit(1)
    try:
        real_flags = args.parse_args(defaults=config_file.default_flags)
    except argparse.ArgumentError as e:
        print(f"Failed to parse command line: {e.message}")
        sys.exit(1)

    replay_mode: bool = False

    # Special utility mode handling
    if real_flags.is_replay():
        assert_no_selection(real_flags, "-pr replay")
        replay_mode = True
    elif real_flags.previous is not None:
        sys.exit(do_process_previous(real_flags))

    # No special modes, proceed with async execution.
    fut = asyncio.ensure_future(
        async_main_wrapper(
            real_flags, config_file=config_file, replay_mode=replay_mode
        )
    )
    try:
        loop = asyncio.get_event_loop()
        loop.run_until_complete(fut)
        sys.exit(fut.result())
    except asyncio.CancelledError:
        print("\n\nReceived interrupt, exiting")
        sys.exit(1)


async def async_main_wrapper(
    flags: args.Flags,
    recorder: event.EventRecorder | None = None,
    config_file: config.ConfigFile | None = None,
    replay_mode: bool = False,
) -> int:
    """Wrapper for the main logic of fx test.

    This wrapper creates a list containing tasks that must be
    awaited before the program exits. The main logic may add tasks to this
    list during execution, and then return the intended status code.

    Args:
        flags (args.Flags): Flags to pass into the main function.
        recorder (event.EventRecorder | None, optional): If set,
            use this event recorder. Used for testing.
        config_file (config.ConfigFile, optional): If set, record
            that this configuration was loaded to set default flags.
        replay_mode (bool, optional): If set, load and replay the most recent log
            instead of running tests.

    Returns:
        The return code of the program.
    """
    tasks: list[asyncio.Task[None]] = []
    if recorder is None:
        recorder = event.EventRecorder()

    end_execution_request_event = asyncio.Event()
    termination_callback_event = asyncio.Event()

    wrapper = AsyncMain(
        flags,
        tasks,
        recorder,
        end_execution_request_event,
        termination_callback_event,
        config_file,
        replay_mode,
    )

    wrapper_task = asyncio.Task(wrapper.main())

    def terminate_handler() -> None:
        end_execution_request_event.set()
        signals.unregister_all_termination_signals()

        def kill_handler() -> None:
            """Immediately cancel and await remaining tasks."""
            wrapper_task.cancel()
            for task in tasks:
                task.cancel()
            print(
                statusinfo.error_highlight(
                    "\nImmediately stopping execution and exiting...",
                    style=flags.style,
                ),
                file=sys.stderr,
            )

        signals.register_on_terminate_signal(kill_handler)

    signals.register_on_terminate_signal(terminate_handler)

    async def terminate_callback_handler() -> None:
        await termination_callback_event.wait()
        wrapper_task.cancel()

    terminate_callback_task = asyncio.Task(terminate_callback_handler())

    try:
        ret = await wrapper_task
    except asyncio.CancelledError:
        ret = 1
    finally:
        terminate_callback_task.cancel()

    if not tasks:
        # Nothing to clean up, return.
        return ret

    # Ensure that queued tasks are cleaned up before returning.
    to_wait = asyncio.Task(asyncio.wait(tasks), name="Drain tasks")
    timeout_seconds = 5
    try:
        await asyncio.wait_for(to_wait, timeout=timeout_seconds)
    except asyncio.TimeoutError:
        print(
            f"\n\nWaiting for tasks to complete for longer than {timeout_seconds} seconds...\n",
            file=sys.stderr,
        )
        wrapper_task.cancel()
        to_wait.cancel()
    except asyncio.CancelledError:
        pass

    return ret


def assert_no_selection(flags: args.Flags, suggested_args: str) -> None:
    """Assert that flags do not have any selections, and print an
       error message if they do.

    Args:
        flags (args.Flags): Command flags
        suggested_args (str): Suggestion for what the user should
            run after executing their tests.
    """
    if flags.selection:
        selection_args = " ".join(flags.selection)
        print(
            f"ERROR: --previous mode does not support running tests, it only displays information from your previous run.\nTry running `fx test {selection_args}` and then `fx test {suggested_args}`"
        )
        sys.exit(1)


def do_process_previous(flags: args.Flags) -> int:
    assert_no_selection(flags, f"-pr {flags.previous}")
    if flags.previous is args.PrevOption.LOG:
        return do_print_logs(flags)
    elif flags.previous is args.PrevOption.PATH:
        env = environment.ExecutionEnvironment.initialize_from_args(
            flags, create_log_file=False
        )
        print(env.get_most_recent_log())
        return 0
    elif flags.previous is args.PrevOption.FAILED_TESTS:
        return do_print_failed(flags)
    elif flags.previous is args.PrevOption.ARTIFACT_PATH:
        env = environment.ExecutionEnvironment.initialize_from_args(
            flags, create_log_file=False
        )
        log_source = log.LogSource.from_env(env)
        # Fast-path: Only parse the artifact_directory_path event, skipping
        # JSON decoding and dataclass deserialization for all other events
        # (such as program_output lines).
        for element in log_source.read_log(
            event_filter=lambda d: isinstance(d.get("payload"), dict)
            and "artifact_directory_path" in d["payload"],
            line_filter=lambda l: '"artifact_directory_path"' in l,
        ):
            if (warning := element.warning) is not None:
                print(f"WARNING: {warning}", file=sys.stderr)
                continue
            if (error := element.error) is not None:
                print(f"ERROR: {error}", file=sys.stderr)
                return 1
            if (event := element.log_event) is None:
                continue
            if event.payload is None:
                continue
            if (artifact_path := event.payload.artifact_directory_path) is None:
                continue
            if artifact_path == "":
                print(
                    "ERROR: The previous run did not specify --artifact-output-directory. Run again with that flag set to get the path.",
                    file=sys.stderr,
                )
                return 1
            if not os.path.isdir(artifact_path):
                print(
                    "ERROR: The artifact directory is missing, it may have been deleted.",
                    file=sys.stderr,
                )
                return 1
            print(artifact_path)
            return 0
        print(
            "ERROR: The previous run is missing an artifact output directory. The log may be corrupt or incomplete.",
            file=sys.stderr,
        )
        return 1
    elif flags.previous is args.PrevOption.HELP:
        print("--previous options:")
        for arg in args.PrevOption:
            prefix = f"{arg:>8}: "
            print(
                "\n".join(
                    textwrap.wrap(
                        prefix + arg.help(),
                        width=70,
                        initial_indent="  ",
                        subsequent_indent="  " + " " * len(prefix),
                    )
                )
                + "\n"
            )
        return 0
    elif flags.previous is args.PrevOption.STATS:
        env = environment.ExecutionEnvironment.initialize_from_args(
            flags, create_log_file=False
        )
        stats = log.compute_stats(log.LogSource.from_env(env))
        print("\nLongest operations:")
        print("-------------------")
        for item in stats.top_n:
            print(f"{item.duration:8.3f}s    {item.label}")

        print("\nSummary:")
        print("--------")
        for category, data in sorted(
            stats.summary.items(), key=lambda x: x[1].sum, reverse=True
        ):
            line = f"{category.value:<14}{data.sum:8.3f}s / {data.count:>3}"
            if data.count > 1:
                line += f"       mean {data.mean:.3f}s (SD {data.std:.3f})"
            print(line)
        return 0
    elif flags.previous is args.PrevOption.STATS_ANALYTICS:
        env = environment.ExecutionEnvironment.initialize_from_args(
            flags, create_log_file=False
        )
        stats = log.compute_stats(log.LogSource.from_env(env))
        print(json.dumps(stats.to_analytics_dict(), separators=(",", ":")))
        return 0
    else:
        print(f"Unknown --previous option {flags.previous}")
        return 1


def do_print_logs(flags: args.Flags) -> int:
    env = environment.ExecutionEnvironment.initialize_from_args(
        flags, create_log_file=False
    )
    if log.pretty_print(log.LogSource.from_env(env)):
        return 0
    else:
        return 1


def do_print_failed(flags: args.Flags) -> int:
    env = environment.ExecutionEnvironment.initialize_from_args(
        flags, create_log_file=False
    )
    log_source = log.LogSource.from_env(env)

    suite_names: dict[event.Id, str] = {}
    failed_test_events: List[event.Event] = []

    def is_suite_event(d: dict[str, typing.Any]) -> bool:
        payload = d.get("payload")
        return isinstance(payload, dict) and (
            "test_suite_started" in payload or "test_suite_ended" in payload
        )

    # Fast-path: Only parse test suite start and end events, skipping
    # JSON decoding and dataclass deserialization for all other events
    # (such as program_output lines).
    for element in log_source.read_log(
        event_filter=is_suite_event,
        line_filter=lambda l: '"test_suite_started"' in l
        or '"test_suite_ended"' in l,
    ):
        if (log_event := element.log_event) is None:
            continue
        if (payload := log_event.payload) is None:
            continue

        if (e := payload.test_suite_started) is not None and log_event.id:
            suite_names[log_event.id] = e.name

        if (
            payload.test_suite_ended is not None
            and payload.test_suite_ended.status != event.TestSuiteStatus.PASSED
        ):
            failed_test_events.append(log_event)

    if len(failed_test_events) == 0:
        print(
            statusinfo.green(
                "The previous run had no failed tests", style=flags.style
            )
        )
    else:
        print("The following tests failed in the previous run:")
        for log_event in failed_test_events:
            if log_event.id and log_event.id in suite_names:
                name = suite_names[log_event.id]
                print(statusinfo.green(f" * fx test {name}", style=flags.style))

    return 0


async def do_replay_log(
    flags: args.Flags, event_signal_for_printer: asyncio.Event
) -> int:
    env = environment.ExecutionEnvironment.initialize_from_args(
        flags, create_log_file=False
    )

    content: list[event.Event] = []
    for log_element in log.LogSource.from_env(env).read_log():
        if log_element.log_event is not None:
            content.append(log_element.log_event)
        elif log_element.warning is not None:
            print(log_element.warning, file=sys.stderr)
        elif log_element.error is not None:
            print(log_element.error, file=sys.stderr)
            return 1

    real_monotonic = time.monotonic()
    if not content:
        print("Log file was empty.", file=sys.stderr)
        return 1
    if content[0].id != event.GLOBAL_RUN_ID:
        print(
            "BUG: Invalid log file, expected to start with a global run identifier.",
            file=sys.stderr,
        )
        return 1
    log_monotonic_start = content[0].timestamp

    def simulated_monotonic_time() -> float:
        real_offset = time.monotonic() - real_monotonic
        return log_monotonic_start + real_offset * flags.replay_speed

    async def wait_until_simulated_time(
        desired_simulated_timestamp: float,
    ) -> None:
        current_simulated_timestamp = simulated_monotonic_time()
        if current_simulated_timestamp < desired_simulated_timestamp:
            simulated_diff = (
                desired_simulated_timestamp - current_simulated_timestamp
            )
            real_diff = simulated_diff / flags.replay_speed
            await asyncio.sleep(real_diff)

    recorder = event.EventRecorder()
    output_future = console.ConsoleOutput(
        monotonic_time_source=simulated_monotonic_time
    ).console_printer(recorder, flags, event_signal_for_printer)

    async def pump_events() -> None:
        test_suite_ids: set[event.Id] = set()
        test_suite_child_ids: set[event.Id] = set()
        for event in content:
            # Wait until it is time to emit this event.
            await wait_until_simulated_time(event.timestamp)

            # Keep track of which programs are part of test suites.
            # We need to override the "print_verbatim" parameter
            # to match the current setting of output for those events.
            if event.parent in test_suite_ids and event.id is not None:
                test_suite_child_ids.add(event.id)
            if (
                event.payload is not None
                and event.payload.test_suite_started is not None
                and event.id is not None
            ):
                test_suite_ids.add(event.id)
            if (
                event.id in test_suite_child_ids
                and event.payload is not None
                and (output_payload := event.payload.program_output) is not None
            ):
                output_payload.print_verbatim = flags.output
            recorder._emit(event)
        recorder.end()

    tasks = [
        asyncio.create_task(pump_events()),
        asyncio.create_task(output_future),
    ]
    await asyncio.wait(tasks)

    print(
        f"\nReplay complete: {len(content)} events from {env.get_most_recent_log()}"
    )

    return 0


def _set_target_nodename(nodename: str | None) -> None:
    """Set or clear FUCHSIA_NODENAME and clear FUCHSIA_NODENAME_IS_FROM_FILE."""
    if nodename:
        os.environ["FUCHSIA_NODENAME"] = nodename
    else:
        os.environ.pop("FUCHSIA_NODENAME", None)
    os.environ.pop("FUCHSIA_NODENAME_IS_FROM_FILE", None)


class AsyncMain:
    _ALL_PACKAGE_MANIFESTS_PATH = [
        "all_package_manifests.list",
    ]

    _PACKAGE_MANIFESTS_FROM_METADATA_PATH = [
        "package_manifests_from_metadata.list",
    ]

    def __init__(
        self,
        flags: args.Flags,
        tasks: list[asyncio.Task[None]],
        recorder: event.EventRecorder,
        end_execution_request_event: asyncio.Event,
        termination_callback_event: asyncio.Event,
        config_file: config.ConfigFile | None = None,
        replay_mode: bool = False,
    ):
        """Wrapper for main logic of fx test

        Args:
            flags (args.Flags): Flags controlling the behavior of fx test.
            tasks (List[asyncio.Tasks]): List to add tasks to that must be awaited before termination.
            recorder (event.Recorder): The recorder for events.
            end_execution_request_event (asyncio.Event): Set by caller to gracefully stop execution.
            termination_callback_event (asyncio.Event): Set by callee to instruct caller to terminate execution.
            config_file (config.ConfigFile, optional): The loaded config, if one was set.
            replay_mode (bool, optional): If set, load and replay the most recent log instead
        """
        self._flags = flags
        self._tasks = tasks
        self._recorder = recorder
        self._config_file = config_file
        self._replay_mode = replay_mode
        self._summary_val: summary.RunSummary = summary.RunSummary()
        self._exec_env: environment.ExecutionEnvironment | None = None
        self._emu_instance_dir: tempfile.TemporaryDirectory[str] | None = None
        self._end_execution_request_event = end_execution_request_event
        self._termination_callback_event = termination_callback_event

        self._package_server_task: asyncio.Task[typing.Any] | None = None
        self._package_server_event: asyncio.Event | None = None
        self._summary_val = summary.RunSummary()
        self._agent_log_dir: str | None = None
        if flags.agent_output:
            import uuid

            self._agent_log_dir = (
                f"/tmp/fx_agent_test_{uuid.uuid4().hex[:8]}/logs"
            )
            os.makedirs(self._agent_log_dir, exist_ok=True)
            self._summary.summary_path = (
                os.path.dirname(self._agent_log_dir) + "/summary.json"
            )
            self._flags.summary_json = self._summary.summary_path

    async def _teardown_resources(self) -> None:
        """Tear down all ephemeral resources (emulator, package server) created by this run."""
        if self._package_server_event and self._package_server_task:
            self._package_server_event.set()
            try:
                await asyncio.wait_for(self._package_server_task, timeout=15.0)
            except TimeoutError:
                self._recorder.emit_warning_message(
                    "Timed out waiting for package server to stop, cancelling task"
                )
                self._package_server_task.cancel()
            self._package_server_event = None
            self._package_server_task = None
        if self._emu_instance_dir is not None:
            await self._teardown_emulator()

    @property
    def _summary(self) -> summary.RunSummary:
        if not hasattr(self, "_summary_val"):
            self._summary_val = summary.RunSummary()
        return self._summary_val

    @_summary.setter
    def _summary(self, val: summary.RunSummary) -> None:
        self._summary_val = val

    def _get_logs_dir(self) -> str | None:
        return summary.get_logs_dir(
            summary_json=self._flags.summary_json,
            artifact_output_directory=self._flags.artifact_output_directory,
        )

    def _emit_summary(self, error: str | None = None) -> None:
        if self._flags.summary_json or self._flags.summary_to_stdout:
            self._summary.finalize(error=error)
            if self._flags.summary_json:
                self._summary.write_to_file(self._flags.summary_json)
            if self._flags.summary_to_stdout:
                print(self._summary.to_json())
            elif self._flags.agent_output:
                print(self._summary.to_markdown())

    async def main(self) -> int:
        """Execute the fx test command through this wrapper.

        Returns:
            The return code of the program.
        """
        try:
            return await self._main_impl()
        finally:
            await asyncio.shield(self._teardown_resources())

    async def _main_impl(self) -> int:
        do_status_output_signal: asyncio.Event = asyncio.Event()
        do_output_to_stdout = (
            self._flags.logpath == args.LOG_TO_STDOUT_OPTION
            or self._flags.summary_to_stdout
        )
        recorder = self._recorder
        flags = self._flags

        async def immediate_exit_handler() -> None:
            await self._end_execution_request_event.wait()
            recorder.emit_warning_message("Interrupt received, terminating.")
            self._termination_callback_event.set()
            recorder.emit_end(error="Terminated due to interrupt")

        # Until the point this task is canceled below, any exit request will request
        # an immediate termination of the command.
        _immediate_exit_task = asyncio.Task(immediate_exit_handler())

        if not do_output_to_stdout and not self._replay_mode:
            self._tasks.append(
                asyncio.create_task(
                    console.ConsoleOutput().console_printer(
                        recorder, flags, do_status_output_signal
                    )
                )
            )
        elif flags.agent_output and not self._replay_mode:
            pass  # Progress reporter suppressed for agents to keep output clean

        # Initialize event recording.
        if not self._replay_mode:
            recorder.emit_init()

        # Try to parse the flags. Emit one event before and another
        # after flag post processing.
        try:
            if self._config_file is not None and self._config_file.is_loaded():
                if not self._replay_mode:
                    recorder.emit_load_config(
                        self._config_file.path or "UNKNOWN PATH",
                        self._config_file.default_flags.__dict__,
                        self._config_file.command_line,
                    )
                recorder.emit_parse_flags(flags.__dict__)
            flags.validate()
            if not self._replay_mode:
                recorder.emit_parse_flags(flags.__dict__)
        except args.FlagError as e:
            if not self._replay_mode:
                recorder.emit_end(f"Flags are invalid: {e}")
            else:
                print(f"Failed to load real flags")
            return 1

        if not do_output_to_stdout:
            recorder.emit_verbatim_message(
                statusinfo.highlight(
                    "Welcome to fx test 🧪\n", style=flags.style
                )
            )

            recorder.emit_instruction_message(
                "Output too verbose? 🤔  Add `--quiet` to `~/.fxtestrc`!\n"
                "See https://fuchsia.dev/fuchsia-src/reference/testing/fx-test for more tips!"
            )

        # Initialize status printing at this point, if desired.
        if flags.status and not do_output_to_stdout:
            do_status_output_signal.set()
            termout.init()

        if self._replay_mode:
            return await do_replay_log(flags, do_status_output_signal)

        # Process and initialize the incoming environment.
        exec_env: environment.ExecutionEnvironment
        try:
            exec_env = environment.ExecutionEnvironment.initialize_from_args(
                flags
            )
        except environment.EnvironmentError as e:
            recorder.emit_end(
                f"Failed to initialize environment: {e}\nDid you run fx set?"
            )
            return 1
        self._exec_env = exec_env
        recorder.emit_process_env(exec_env)

        flags.update_artifacts_directory_with_out_path(
            os.path.abspath(exec_env.out_dir)
        )

        recorder.emit_artifact_directory_path(
            os.path.abspath(flags.artifact_output_directory)
            if flags.artifact_output_directory
            else None
        )

        if (
            flags.artifact_output_directory
            and not flags.timestamp_artifacts
            and not set(sys.argv).intersection(
                set(["--timestamp-artifacts", "--no-timestamp-artifacts"])
            )
        ):
            recorder.emit_warning_message(
                "You have not specified --[no-]timestamp-artifacts.\nArtifact output will overwrite previous runs.\nThe default will soon change to support timestamped directories."
            )
            recorder.emit_instruction_message(
                "Specify --timestamp-artifacts to output in timestamped subdirectories. This will soon become the default."
            )

        # Configure file logging based on flags.
        if flags.log and exec_env.log_file:
            output_file: typing.TextIO
            if exec_env.log_to_stdout():
                output_file = sys.stdout
            else:
                output_file, actual_log_path = log.open_exclusive_log_file(
                    exec_env.log_file
                )
                if actual_log_path != exec_env.log_file:
                    recorder.emit_warning_message(
                        f"Log file '{exec_env.log_file}' is in use by another running test process. "
                        f"Writing this run's logs to '{actual_log_path}' instead."
                    )
                    exec_env.log_file = actual_log_path

                if flags.save_log_path_to_file:
                    try:
                        with open(flags.save_log_path_to_file, "w") as pf:
                            pf.write(os.path.abspath(actual_log_path))
                            pf.flush()
                    except OSError as e:
                        recorder.emit_warning_message(
                            f"Failed to write log path to {flags.save_log_path_to_file}: {e}"
                        )
            self._tasks.append(
                asyncio.create_task(log.writer(recorder, output_file))
            )

        # Validate output directory
        if (
            flags.artifact_output_directory
            and os.path.exists(flags.artifact_output_directory)
            and len(os.listdir(flags.artifact_output_directory)) > 0
        ):
            recorder.emit_end(
                f"Your output directory already exists and is not empty.\nUse --timestamp-artifacts to create new subdirectories for each run, and use `fx test --prev artifact-path` to get the path from the previous run.",
            )
            return 1

        if flags.show_affected_tests:
            await find_affected.show_affected_tests(exec_env, flags, recorder)
            return 0

        if flags.run_affected_tests:
            targets = await find_affected.get_affected_targets(
                exec_env, flags.affected_since, recorder
            )
            if targets:
                labels = await self._add_affected_tests(
                    targets, exec_env, recorder
                )
                if labels is None:
                    recorder.emit_end(
                        "Failed to add affected tests to build graph"
                    )
                    return 1
                if flags.selection is None:
                    flags.selection = []
                for label in labels:
                    if label not in flags.selection:
                        flags.selection.append(label)
            else:
                recorder.emit_info_message("\nNo affected tests found to run.")
                return 0

        # Load the list of tests to execute.
        try:
            tests = await self._load_test_list()
        except Exception as e:
            recorder.emit_end(f"Failed to load tests: {e}")
            return 1

        # Use flags to select which tests to run.
        try:
            mode = selection.SelectionMode.ANY
            if flags.host:
                mode = selection.SelectionMode.HOST
            elif flags.device:
                mode = selection.SelectionMode.DEVICE
            elif flags.only_e2e:
                mode = selection.SelectionMode.E2E
            selections = await selection.select_tests(
                tests,
                flags.selection,
                exec_env,
                mode,
                flags.fuzzy,
                recorder=recorder,
                exact_match=flags.exact,
            )
            # Mutate the selections based on the command line flags.
            selections.apply_flags(flags)
            if len(selections.selected_but_not_run) != 0:
                total_count = len(selections.selected) + len(
                    selections.selected_but_not_run
                )
                recorder.emit_info_message(
                    f"Selected {total_count} tests, but only running {len(selections.selected)} due to flags."
                )
            recorder.emit_test_selections(selections)
        except selection.SelectionError as e:
            recorder.emit_end(f"Selection is invalid: {e}")
            return 1
        except RuntimeError as e:
            recorder.emit_end(
                f"There was an internal error calling the selection matcher program: {e}"
            )
            return 1

        # Check that the selected tests are valid.
        try:
            await self._validate_test_selections(selections)
        except self._SelectionValidationError as e:
            self._emit_summary(error=str(e))
            recorder.emit_end(str(e))
            return 1

        # Don't actually run any tests if --dry was specified, instead just
        # print which tests were selected and exit.
        if flags.dry:
            recorder.emit_verbatim_message("Selected the following tests:")
            for s in selections.selected:
                recorder.emit_verbatim_message(f"  {s.name()}")
            recorder.emit_instruction_message(
                "\nWill not run any tests, --dry specified"
            )
            recorder.emit_end()
            return 0

        need_emulator = False
        emulator_started = False
        if selections.has_device_test() and not await self._has_active_device():
            need_emulator = True

        async def end_execution(
            error: str | None = None, id: event.Id | None = None
        ) -> None:
            await self._teardown_resources()
            recorder.emit_end(error=error, id=id)
            self._emit_summary(error=error)

        # If enabled, try to build and update the selected tests.
        if flags.build and not await self._do_build(selections):
            await end_execution("Failed to build.")
            return 1

        if flags.updateifinbase and self._has_tests_in_base(selections):
            recorder.emit_info_message(f"\nBuilding update package.")
            recorder.emit_instruction_message(
                "Use --no-updateifinbase to skip updating base packages."
            )
            build_id = recorder.emit_build_start(
                targets=["//build/images/updates"]
            )
            output = await run_build(
                exec_env,
                ["//build/images/updates"],
                recorder=recorder,
                parent=build_id,
                abort_signal=self._end_execution_request_event,
            )
            if output is None or output.return_code != 0:
                error = _emit_build_failure(recorder, output)
                recorder.emit_end(error, id=build_id)
                await end_execution(f"Failed to build update package: {error}")
                return 1
            recorder.emit_end(id=build_id)
            recorder.emit_info_message(
                "\nRunning an OTA before executing tests"
            )
            ota_result = await execution.run_command(
                *exec_env.fx_cmd_line("ota", "--no-build"),
                recorder=recorder,
                print_verbatim=True,
            )
            if ota_result is None or ota_result.return_code != 0:
                recorder.emit_warning_message(
                    "OTA failed, attempting to run tests anyway"
                )

        if not self._validate_package_merkle_hashes(selections):
            await end_execution("Failed to validate package Merkle hashes.")
            return 1

        if need_emulator and flags.allow_temporary_emulator:
            if not await self._start_emulator():
                await end_execution("Failed to start emulator.")
                return 1
            emulator_started = True

        # If there is exactly one active device and no target is explicitly specified,
        # set it as the default target via FUCHSIA_NODENAME so that all child commands
        # (like the package server and tests) target this device.
        if selections.has_device_test() and not os.environ.get(
            "FUCHSIA_NODENAME"
        ):
            await self._bind_to_active_device()

        if not flags.list_runtime_deps:
            package_server_behavior = (
                await self._check_if_package_server_needed(selections, exec_env)
            )
            match package_server_behavior:
                case self._PackageServerBehavior.FAIL:
                    return 1
                case self._PackageServerBehavior.PRESENT:
                    pass
                case self._PackageServerBehavior.START:
                    self._start_package_server()

        if selections.has_device_test() and not flags.list_runtime_deps:
            recorder.emit_info_message("Waiting for repository registration...")
            if await self._wait_for_repository_registration():
                recorder.emit_info_message(
                    "Repository registered successfully!"
                )
            else:
                recorder.emit_warning_message(
                    "Timeout waiting for repository registration. Tests may fail to resolve package URLs."
                )

        # Generate a new test-list.json file based on the built tests.
        try:
            test_list_entries = await self._generate_test_list()
        except (ValueError, RuntimeError) as e:
            await end_execution(
                f"Failed to generate and load test-list.json: {e}"
            )
            return 1

        try:
            test_list_file.Test.augment_tests_with_info(
                selections.selected, test_list_entries
            )
        except ValueError as e:
            await end_execution(
                f"Generated test-list.json is inconsistent: {e}.\nThis is a bug."
            )
            return 1

        # Don't actually run tests if --list was specified, instead gather the
        # list of test cases for each test and output to the user.
        if flags.list:
            recorder.emit_info_message("Enumerating all test cases...")
            recorder.emit_instruction_message(
                "Will not run any tests, --list specified"
            )
            enumeration_result = await self._enumerate_test_cases(selections)
            await end_execution()
            return enumeration_result

        if flags.list_runtime_deps:
            recorder.emit_info_message("Listing runtime_deps for all tests...")
            recorder.emit_instruction_message(
                "Will not run any tests, --list-host-test-data specified"
            )
            self._list_runtime_deps(selections)
            await end_execution()
            return 0

        # From this point on, separately handle exit requests so that tests can
        # close cleanly.
        _immediate_exit_task.cancel()
        if self._end_execution_request_event.is_set():
            # We raced with an exit request.
            # Request termination and await exit.
            self._termination_callback_event.set()
            await asyncio.sleep(3600)

        # Finally, run all selected tests.
        if not await self._run_all_tests(selections):
            if not flags.debugger_will_attach() and not flags.host:
                # Note: it is important that we put --break-on-failure before the rest of the command
                # line arguments so that it is ensured that this fx test argument comes before any extra
                # arguments are passed through to the test executable (e.g. after "--").
                msg = (
                    "To debug with zxdb: fx test --break-on-failure {}".format(
                        " ".join(sys.argv[1:])
                    )
                )

                if agents_lib.is_invoked_by_agent():
                    msg = "To debug with fx debug cli: fx test --agent-debugging-mode {}".format(
                        " ".join(sys.argv[1:])
                    )

                self._summary.hints.append(msg)
                recorder.emit_instruction_message(msg)

            await end_execution("Failed to run all tests")

            return 1

        await end_execution()
        return 0

    async def _add_affected_tests(
        self,
        targets: list[find_affected.AffectedTarget],
        exec_env: environment.ExecutionEnvironment,
        recorder: event.EventRecorder,
    ) -> list[str] | None:
        """Runs fx add-test commands for affected targets and returns their labels.

        Returns:
            The list of test labels to add to selection.
        """
        recorder.emit_instruction_message(
            f"\nAdding {len(targets)} affected test(s) to build graph..."
        )
        device_targets = [t.pure_label for t in targets if not t.is_host]
        host_targets = [t.pure_label for t in targets if t.is_host]

        if device_targets:
            cmd = exec_env.fx_cmd_line("add-test", *device_targets)
            res = await execution.run_command(*cmd, recorder=recorder)
            if res is None or res.return_code != 0:
                recorder.emit_warning_message(
                    f"\nFailed to add affected test(s) to build graph: {' '.join(device_targets)}"
                )
                return None

        if host_targets:
            cmd = exec_env.fx_cmd_line("add-host-test", *host_targets)
            res = await execution.run_command(*cmd, recorder=recorder)
            if res is None or res.return_code != 0:
                recorder.emit_warning_message(
                    f"\nFailed to add affected host test(s) to build graph: {' '.join(host_targets)}"
                )
                return None

        return [t.pure_label for t in targets]

    async def _load_test_list(
        self,
    ) -> list[test_list_file.Test]:
        """Load the input files listing tests and parse them into a list of Tests.

        Raises:
            TestFileError: If the tests.json file is invalid.
            DataParseError: If data could not be deserialized from JSON input.
            JSONDecodeError: If a JSON file fails to parse.
            IOError: If a file fails to open.
            ValueError: If the tests.json and test-list.json files are
                incompatible for some reason.

        Returns:
            list[test_list_file.Test]: List of available tests to execute.
        """

        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None

        # Load the tests.json file.
        parse_id: event.Id | None = None
        try:
            parse_id = recorder.emit_start_file_parsing(
                exec_env.relative_to_root(exec_env.test_json_file),
                exec_env.test_json_file,
            )
            test_file_entries: list[
                tests_json_file.TestEntry
            ] = tests_json_file.TestEntry.from_file(exec_env.test_json_file)
            recorder.emit_test_file_loaded(
                test_file_entries, exec_env.test_json_file
            )
            recorder.emit_end(id=parse_id)
        except (
            tests_json_file.TestFileError,
            json.JSONDecodeError,
            IOError,
        ) as e:
            recorder.emit_end("Failed to parse: " + str(e), id=parse_id)
            raise e

        # Wrap contents of test.json in PartialTest, which supports matching but
        # still needs a lazily created test-list.json to be a full Test.
        try:
            tests = list(map(test_list_file.Test, test_file_entries))
            return tests
        except ValueError as e:
            recorder.emit_end(
                f"tests.json and test-list.json are inconsistent: {e}"
            )
            raise e

    async def _generate_test_list(
        self,
    ) -> dict[str, test_list_file.TestListEntry]:
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None

        with tempfile.TemporaryDirectory() as td:
            out_path = os.path.join(td, "test-list.json")
            result = await execution.run_command(
                *exec_env.fx_cmd_line(
                    "test_list_tool",
                    "--build-dir",
                    exec_env.out_dir,
                    "--input",
                    exec_env.test_json_file,
                    "--disabled-ctf-tests",
                    exec_env.disabled_ctf_tests_file,
                    "--output",
                    out_path,
                    "--test-components",
                    os.path.join(exec_env.out_dir, "test_components.json"),
                    "--ignore-device-test-errors",
                ),
                recorder=recorder,
            )
            if result is None or result.return_code != 0:
                suffix = ""
                if result is not None:
                    suffix = f":\n{result.stdout}\n{result.stderr}"
                raise RuntimeError(
                    f"Could not generate a new test-list.json{suffix}"
                )

            exec_env.test_list_file = out_path

            # Load the generated test-list.json file.
            try:
                parse_id = recorder.emit_start_file_parsing(
                    exec_env.relative_to_root(exec_env.test_list_file),
                    exec_env.test_list_file,
                )
                test_list_entries = (
                    test_list_file.TestListFile.entries_from_file(
                        exec_env.test_list_file
                    )
                )
                recorder.emit_end(id=parse_id)
                return test_list_entries
            except (
                dataparse.DataParseError,
                json.JSONDecodeError,
                IOError,
            ) as e:
                raise e

    class _SelectionValidationError(Exception):
        """A problem occurred when validating test selections.

        The message contains a human-readable explanation of the problem.
        """

    async def _validate_test_selections(
        self,
        selections: selection_types.TestSelections,
    ) -> None:
        """Validate the selections matched from tests.json.

        Args:
            selections (TestSelections): The selection output to validate.

        Raises:
            SelectionValidationError: If the selections are invalid.
        """

        recorder = self._recorder
        flags = self._flags
        exec_env = self._exec_env
        assert exec_env is not None

        missing_groups: list[selection_types.MatchGroup] = []

        for group, matches in selections.group_matches:
            if not matches:
                missing_groups.append(group)

        if missing_groups:
            recorder.emit_warning_message(
                "\nCould not find any tests to run for at least one set of arguments you provided."
            )
            missing_group_with_name = next(
                filter(lambda x: len(x.names) > 0, missing_groups), None
            )
            if flags.exact and missing_group_with_name is not None:
                recorder.emit_instruction_message(
                    f" --exact does not match packages or components by default\n Did you mean: --exact --package {missing_group_with_name}?"
                )
            recorder.emit_info_message(
                "\nMake sure this test is transitively in your 'fx set' arguments."
            )
            recorder.emit_info_message(
                "See https://fuchsia.dev/fuchsia-src/development/testing/faq for more information."
            )

            if flags.show_suggestions:

                def suggestion_args(
                    arg: str, threshold: float | None = None
                ) -> list[str]:
                    suggestion_args = [
                        "search-tests",
                        f"--max-results={flags.suggestion_count}",
                        "--color" if flags.style else "--no-color",
                    ]
                    if flags.host:
                        suggestion_args.append("--host")
                    if flags.device:
                        suggestion_args.append("--device")
                    suggestion_args.append(arg)
                    if threshold is not None:
                        suggestion_args += ["--threshold", str(threshold)]
                    if flags.remote_suggestions:
                        suggestion_args += ["--remote"]
                    for builder in flags.remote_suggestion_builder:
                        suggestion_args += ["--builder", builder]
                    return exec_env.fx_cmd_line(*suggestion_args)

                arg_threshold_pairs = []
                for group in missing_groups:
                    # Create pairs of a search string and threshold.
                    # Thresholds depend on the number of arguments joined.
                    # We have only a single search field, so we concatenate
                    # the names into one big group.  To correct for lower
                    # match thresholds due to this union, we adjust the
                    # threshold when there is more than a single value to
                    # match against.
                    all_args = group.names.union(group.components).union(
                        group.packages
                    )
                    arg_threshold_pairs.append(
                        (
                            ",".join(list(all_args)),
                            (
                                max(0.4, 0.9 - len(all_args) * 0.05)
                                if len(all_args) > 1
                                else None
                            ),
                        ),
                    )

                outputs = await run_commands_in_parallel(
                    [
                        suggestion_args(arg_pair[0], arg_pair[1])
                        for arg_pair in arg_threshold_pairs
                    ],
                    "Find suggestions",
                    recorder=recorder,
                    maximum_parallel=10,
                )

                for group, output in zip(missing_groups, outputs):
                    if output is not None and output.stdout:
                        group_suggestions = (
                            summary.parse_suggestions_from_output(output.stdout)
                        )
                        for s in group_suggestions:
                            if s not in self._summary.suggestions:
                                self._summary.suggestions.append(s)
                        recorder.emit_verbatim_message(
                            f"\nFor `{group}`, did you mean any of the following?\n"
                        )
                        recorder.emit_verbatim_message(output.stdout)

        if not selections.selected:
            if missing_groups:
                raise self._SelectionValidationError(
                    "No tests found for the following selections:\n "
                    + "\n ".join([str(m) for m in missing_groups])
                )
            else:
                raise self._SelectionValidationError(
                    "No tests found matching criteria."
                )
        elif not flags.allow_empty_selection and missing_groups:
            raise self._SelectionValidationError(
                "No tests found for the following selections:\n "
                + "\n ".join([str(m) for m in missing_groups])
            )
        boot_tests = [
            test.name() for test in selections.selected if test.is_boot_test()
        ]
        if boot_tests:
            tests_str = ", ".join(boot_tests)
            raise self._SelectionValidationError(
                f"Boot tests are not supported by `fx test`. Use `fx run-boot-test` or `fx core-tests`:\n  {tests_str}"
            )

        if flags.selection and not flags.e2e:
            e2e_tests = [
                test.name()
                for test in selections.selected
                if test.is_e2e_test()
            ]
            if e2e_tests:
                tests_str = ", ".join(e2e_tests)
                raise self._SelectionValidationError(
                    f"The following tests are e2e tests, but the --e2e flag was not provided:\n  {tests_str}\n"
                    + "Please pass --e2e to run e2e tests."
                )

    def _validate_package_merkle_hashes(
        self,
        selections: selection_types.TestSelections,
    ) -> bool:
        """Validate that all selected device tests have valid Merkle hashes in the repository.

        Args:
            selections (TestSelections): The selections to validate.

        Returns:
            bool: True if validation succeeded, False if any Merkle hash is missing.
        """
        if not self._flags.use_package_hash or self._flags.list_runtime_deps:
            return True

        device_tests = [
            test for test in selections.selected if test.is_pure_device_test()
        ]
        if not device_tests:
            return True

        if self._exec_env is None:
            return False

        try:
            package_repo = package_repository.PackageRepository.from_env_cached(
                self._exec_env
            )
        except package_repository.PackageRepositoryError as e:
            self._recorder.emit_warning_message(
                f"Could not load package repository ({str(e)})"
                f"{package_repository.MERKLE_ERROR_HELP_SUFFIX}"
            )
            return False

        has_error = False
        for test in device_tests:
            url = test.build.test.package_url
            if not url:
                continue
            try:
                package_repo.resolve_component_url(url)
            except package_repository.PackageRepositoryError as e:
                self._recorder.emit_warning_message(str(e))
                has_error = True

        return not has_error

    async def _do_build(
        self,
        tests: selection_types.TestSelections,
    ) -> bool:
        """Attempt to build the selected tests.

        Args:
            tests (selection.TestSelections): Tests to attempt to build.

        Returns:
            bool: True only if the tests were built and published, False otherwise.
        """
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None
        allow_build_updates = self._flags.build_updates
        error: str | None = None

        # Bazel labels start with @@ and have no toolchain suffix.
        # GN Labels start with // and end with a toolchain, starting with
        # '('. Both toolchain and '//' need to be omitted for building
        # device tests through fx build.
        gn_label_to_rule = re.compile(r"//([^()]+)\((.+)\)")
        build_bazel_targets: list[str] = []
        build_gn_targets_by_toolchain: defaultdict[
            str, list[str]
        ] = defaultdict(list)
        for selection in tests.selected:
            test_label = selection.build.test.label
            if test_label.startswith("@@"):
                build_bazel_targets.append(test_label)
                continue

            label = selection.build.test.package_label or test_label
            match = gn_label_to_rule.match(label)

            if match:
                target = match.group(1)
                toolchain = match.group(2)
                assert isinstance(toolchain, str)

                build_gn_targets_by_toolchain[toolchain].append(f"//{target}")
            else:
                recorder.emit_warning_message(f"Unknown entry {selection}")
                return False

        # First build a command line to build GN targets, if needed.
        build_command_line: list[str] = []
        for key, vals in sorted(list(build_gn_targets_by_toolchain.items())):
            if "fuchsia:" in key:
                # Use --default instead of fuchsia toolchains, otherwise some
                # ZBI tests fail to build.
                build_command_line.append("--default")
            else:
                build_command_line.append(f"--toolchain={key}")
            build_command_line.extend(vals)

        if tests.has_e2e_test() and allow_build_updates:
            build_command_line.extend(["--default", "//build/images/updates"])
            recorder.emit_instruction_message(
                "E2E test selected, building updates package"
            )
        elif tests.has_device_test():
            missing_packages: list[str] = []
            try:
                package_repo = package_repository.PackageRepository.from_env(
                    exec_env
                )
                device_test_names = [
                    package_name
                    for test in tests.selected
                    if (package_name := test.package_name()) is not None
                ]

                missing_packages = sorted(
                    set(
                        filter(
                            lambda name: name
                            not in package_repo.name_to_merkle,
                            device_test_names,
                        )
                    )
                )
            except package_repository.PackageRepositoryError:
                missing_packages = [
                    package_name
                    for test in tests.selected
                    if (package_name := test.package_name()) is not None
                ]

            if missing_packages and allow_build_updates:
                recorder.emit_info_message(
                    f"Missing {len(missing_packages)} from package lists. Regenerating test lists and building updates."
                )
                packages_string = "   - " + "\n   - ".join(
                    missing_packages[:10]
                )
                if len(missing_packages) > 10:
                    packages_string += (
                        f"\n   (and {len(missing_packages) - 10} more)"
                    )
                recorder.emit_instruction_message(packages_string)
                # Rebuild lightweight package lists and discoverable metadata so
                # the added test packages can be published without requiring a full build.
                build_command_line.extend(
                    [
                        "--default",
                        "//build/images/updates:package_lists",
                        "//build/images/updates:discoverable_manifests_from_metadata.list",
                        "//build/images/updates:prepare_publish",
                    ]
                )

        build_id = recorder.emit_build_start(targets=build_command_line)
        recorder.emit_instruction_message("Use --no-build to skip building")

        recorder.emit_info_message("\nExecuting build.")

        await asyncio.sleep(0.1)

        logs_dir = self._get_logs_dir()
        build_log_path = (
            os.path.join(logs_dir, "build.log") if logs_dir else None
        )

        if build_command_line:
            build_output = await run_build(
                exec_env,
                build_command_line,
                recorder=self._recorder,
                parent=build_id,
                abort_signal=self._end_execution_request_event,
            )

            if build_log_path and build_output:
                if build_log_path:
                    os.makedirs(os.path.dirname(build_log_path), exist_ok=True)
                with open(build_log_path, "w") as f:
                    stdout = getattr(build_output, "stdout", "")
                    stderr = getattr(build_output, "stderr", "")
                    f.write(stdout if stdout else "")
                    if stderr:
                        f.write("\n" + stderr)

            if build_output is None or build_output.return_code != 0:
                error = _emit_build_failure(recorder, build_output)
                self._summary.build = summary.BuildResult(
                    status="FAILED",
                    exit_code=build_output.return_code if build_output else -1,
                    log_path=build_log_path,
                    error=error,
                )
                recorder.emit_end(error, id=build_id)
                return False
            else:
                self._summary.build = summary.BuildResult(
                    status="PASSED",
                    exit_code=build_output.return_code,
                    log_path=build_log_path,
                )

        # Second, launch another command line to build and export Bazel host tests
        if build_bazel_targets:
            build_output = await run_build(
                exec_env,
                ["--host", "--quiet"] + build_bazel_targets,
                recorder=self._recorder,
                parent=build_id,
                abort_signal=self._end_execution_request_event,
            )

            if build_output is None or build_output.return_code != 0:
                error = _emit_build_failure(recorder, build_output)
                self._summary.build = summary.BuildResult(
                    status="FAILED",
                    exit_code=build_output.return_code if build_output else -1,
                    log_path=build_log_path,
                    error=error,
                )
                recorder.emit_end(error, id=build_id)
                return False
            else:
                self._summary.build = summary.BuildResult(
                    status="PASSED",
                    exit_code=build_output.return_code,
                    log_path=build_log_path,
                )

        if tests.has_device_test():
            try:
                await self._publish_packages(build_id)
            except self._PublishException as e:
                error = e.reason

        package_repository.PackageRepository.from_env_cached.cache_clear()

        if not error and not await self._post_build_checklist(tests, build_id):
            error = "Post build checklist failed"

        recorder.emit_end(error, id=build_id)

        return error is None

    @dataclass
    class _PublishException(Exception):
        """Exception that is raised if we fail to publish packages."""

        reason: str

    async def _publish_packages(self, build_id: event.Id) -> None:
        """Publish packages that were just built.

        Args:
            build_id (event.Id): The event of the parent build to nest events under.

        Raises:
            self._PublishException: If publishing fails for any reason.
        """
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None

        amber_directory = os.path.join(exec_env.out_dir, "amber-files")
        delivery_blob_type = self._read_delivery_blob_type()

        # This manifest file is updated only following the build
        # process, which poses a problem when we want to use `fx add-test`
        # and then expect the test to work without a full
        # rebuild. To solve this problem, we actually synthesize a
        # new package manifest consisting of the original
        # all_package_manifests.list plus any new packages listed in
        # the generated file "package_manifests_from_metadata.list".
        # The new combined file will contain tests added using `fx add-test`.
        all_package_manifests = os.path.join(
            exec_env.out_dir,
            *self._ALL_PACKAGE_MANIFESTS_PATH,
        )

        version = "1"
        manifest_list: list[str] = []
        if os.path.isfile(all_package_manifests):
            try:
                with open(all_package_manifests) as f:
                    package_manifest = json.load(f)
                manifest_list = package_manifest.get("content", {}).get(
                    "manifests", []
                )
                version = package_manifest.get("version", "1")
            except Exception:
                raise self._PublishException(
                    "BUG: Failed to load manifest list from all_package_manifests.list\nPlease file a bug."
                )

        # Load the generated list file (package_manifests_from_metadata.list).
        manifests_metadata_path = os.path.abspath(
            os.path.join(
                exec_env.out_dir,
                *self._PACKAGE_MANIFESTS_FROM_METADATA_PATH,
            )
        )

        # Read all entries from the generated file, merging them back into the package manifest.
        if os.path.isfile(manifests_metadata_path):
            with open(manifests_metadata_path) as f:
                try:
                    data = json.load(f)
                    meta_manifests = data.get("content", {}).get(
                        "manifests", []
                    )
                    if "version" in data:
                        version = data["version"]
                except json.JSONDecodeError:
                    meta_manifests = [
                        stripped_line
                        for l in f.readlines()
                        if (stripped_line := l.strip()) != ""
                    ]
            manifest_list = list(set(manifest_list).union(meta_manifests))

        package_manifest = {
            "content": {"manifests": sorted(manifest_list)},
            "version": version or "1",
        }

        with tempfile.TemporaryDirectory() as td:
            # Generate a merged temporary manifest.
            temp_manifest_path = os.path.join(td, "temp_manifest.list")
            with open(temp_manifest_path, "w") as f:
                json.dump(package_manifest, f)

            # Publish the packages listed in the merged manifest.
            publish_args = (
                exec_env.fx_cmd_line(
                    "ffx",
                    "repository",
                    "publish",
                    "--trusted-root",
                    os.path.abspath(
                        os.path.join(amber_directory, "repository/root.json")
                    ),
                    "--ignore-missing-packages",
                    "--time-versioning",
                )
                + (
                    ["--delivery-blob-type", str(delivery_blob_type)]
                    if delivery_blob_type is not None
                    else []
                )
                + [
                    "--package-list",
                    temp_manifest_path,
                    os.path.abspath(amber_directory),
                ]
            )

            output = await execution.run_command(
                *publish_args,
                recorder=recorder,
                parent=build_id,
                print_verbatim=True,
                env={"CWD": exec_env.out_dir},
            )
            if not output:
                raise self._PublishException("Failure publishing packages.")
            elif output.return_code != 0:
                raise self._PublishException(
                    f"Publish returned non-zero exit code {output.return_code}"
                )

    def _read_delivery_blob_type(
        self,
    ) -> int | None:
        """Read the delivery blob type from the output directory.

        The delivery_blob_config.json file contains a "type" field that must
        be passed along to package publishing if set.

        This functions attempts to load the file and returns the value of that
        field if set.

        Returns:
            int | None: The delivery blob type, if found. None otherwise.
        """
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None

        expected_path = os.path.join(
            exec_env.out_dir, "delivery_blob_config.json"
        )
        id = recorder.emit_start_file_parsing(
            "delivery_blob_config.json", expected_path
        )
        if not os.path.isfile(expected_path):
            recorder.emit_end(
                error="Could not find delivery_blob_config.json in output",
                id=id,
            )
            return None

        with open(expected_path) as f:
            val: dict[str, typing.Any] = json.load(f)
            recorder.emit_end(id=id)
            return int(val["type"]) if "type" in val else None

    def _has_tests_in_base(
        self,
        tests: selection_types.TestSelections,
    ) -> bool:
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None

        base_file = os.path.join(exec_env.out_dir, "base_packages.list")
        parse_id = recorder.emit_start_file_parsing(
            "base_packages.list", base_file
        )

        base_package_names: list[str]
        try:
            with open(base_file) as f:
                contents = json.load(f)
            base_package_names = contents["content"]["names"]
        except (json.JSONDecodeError, KeyError) as e:
            recorder.emit_end(f"Parsing file failed: {e}", id=parse_id)
            raise e
        except FileNotFoundError:
            # No base packages found
            recorder.emit_end(id=parse_id)
            return False

        test_packagess_in_base = [
            name
            for t in tests.selected
            if (name := t.package_name()) in base_package_names
        ]

        if test_packagess_in_base:
            names = ", ".join(test_packagess_in_base[:3])
            tests_are_in_base_including = (
                "tests are in base, including"
                if len(test_packagess_in_base) > 1
                else "test is in base:"
            )
            recorder.emit_info_message(
                f"\n{len(test_packagess_in_base)} {tests_are_in_base_including} {names}"
            )

        recorder.emit_end(id=parse_id)

        return bool(test_packagess_in_base)

    async def _post_build_checklist(
        self,
        tests: selection_types.TestSelections,
        build_id: event.Id,
    ) -> bool:
        """Perform a number of post-build checks to ensure we are ready to run tests.

        Args:
            tests (selection.TestSelections): Tests selected to run.
            build_id (event.Id): ID of the build event to use at the parent of any operations executed here.

        Returns:
            bool: True only if post-build checks passed, False otherwise.
        """
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None

        if tests.has_device_test():
            try:
                if self._has_tests_in_base(tests):
                    recorder.emit_info_message(
                        "Some selected test(s) are in the base package set. Running an OTA."
                    )
                    output = await execution.run_command(
                        *exec_env.fx_cmd_line("ota"),
                        recorder=recorder,
                        print_verbatim=True,
                    )
                    if not output or output.return_code != 0:
                        recorder.emit_warning_message("OTA failed")
                        return False
            except (IOError, json.JSONDecodeError, KeyError):
                return False

        return True

    class _PackageServerBehavior(enum.Enum):
        # The package server is present, continue.
        PRESENT = 1
        # The package server is not present, fail.
        FAIL = 2
        # The package server is not present, start a temporary one.
        START = 3

    async def _check_if_package_server_needed(
        self,
        tests: selection_types.TestSelections,
        exec_env: environment.ExecutionEnvironment,
    ) -> _PackageServerBehavior:
        flags = self._flags
        recorder = self._recorder
        if (
            tests.has_device_test()
            and not await has_package_server_connected_to_device(
                exec_env,
                recorder,
            )
        ):
            if not flags.allow_temporary_package_server:
                recorder.emit_instruction_message(
                    "\nYou do not seem to have a package server running, but you have selected at least one device test.\nEnsure that you have `fx serve` running and that you have selected your desired device using `fx set-device`.\n"
                )
                recorder.emit_end("Could not find a running package server.")
                return self._PackageServerBehavior.FAIL
            else:
                recorder.emit_instruction_message(
                    "\nYou do not seem to have a package server running. A temporary one will be started for the duration of this execution."
                )
                return self._PackageServerBehavior.START
        else:
            return self._PackageServerBehavior.PRESENT

    async def _run_all_tests(
        self,
        tests: selection_types.TestSelections,
    ) -> bool:
        """Execute all selected tests.

        Args:
            tests (selection.TestSelections): The selected tests to run.

        Returns:
            bool: True only if all tests ran successfully, False otherwise.
        """
        flags = self._flags
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None

        max_parallel = flags.parallel

        # This is an error since no tests that were selected involved the device, even if the --host
        # flag was not specified on the command line. If a test selection includes _some_ device tests,
        # those are allowed. The existence of host tests among device tests is not a problem for the
        # debugger integration, but users may be confused if they try to debug host tests with
        # automatic selection.
        if not tests.has_device_test() and flags.debugger_will_attach():
            recorder.emit_warning_message(
                "\n--break-on-failure and --breakpoint flags are not supported with host tests."
            )
            recorder.emit_instruction_message(
                "\nRemove the --break-on-failure and --breakpoint flags to run these host-only tests."
            )
            return False

        device_environment: environment.DeviceEnvironment | None = None
        if tests.has_device_test():
            try:
                device_environment = (
                    await execution.get_device_environment_from_exec_env(
                        exec_env, recorder=recorder
                    )
                )
            except execution.DeviceConfigError as e:
                # Allow missing device configuration error if we don't have end
                # to end tests. This allows us to run against devices that don't
                # have an SSH address (like USB and vsock).
                # TODO(https://fxbug.dev/417777659): Remove this allowance once
                # we have a good strategy for how to forward non-ssh-connected
                # devices.
                if tests.has_e2e_test():
                    raise e

        test_group = recorder.emit_test_group(len(tests.selected) * flags.count)

        @dataclass
        class ExecEntry:
            """Wrapper for test executions to share a signal for aborting by groups."""

            # The test execution to run.
            exec: execution.TestExecution

            # Signal for aborting the execution of a specific group of tests,
            # including this one.
            abort_group: asyncio.Event

        @dataclass
        class RunState:
            total_running: int = 0
            non_hermetic_running: int = 0
            hermetic_test_queue: asyncio.Queue[ExecEntry] = field(
                default_factory=lambda: asyncio.Queue()
            )
            non_hermetic_test_queue: asyncio.Queue[ExecEntry] = field(
                default_factory=lambda: asyncio.Queue()
            )

        run_condition = asyncio.Condition()
        run_state = RunState()

        for test in tests.selected:
            execs = [
                execution.TestExecution(
                    test,
                    exec_env,
                    flags,
                    run_suffix=None if flags.count == 1 else i + 1,
                    device_env=(
                        None if not test.needs_device() else device_environment
                    ),
                )
                for i in range(flags.count)
            ]

            # Shared event group for all repetitions of a test.
            abort_group = asyncio.Event()

            for exec in execs:
                if not flags.fail_by_group:
                    # The --fail-by-group flag ensures that all execution of a test will
                    # fail if a single execution fails.
                    #
                    # If it is not set, ensure that each execution has its own abort_group, allowing
                    # other runs to keep going.

                    abort_group = asyncio.Event()
                if exec.is_hermetic():
                    run_state.hermetic_test_queue.put_nowait(
                        ExecEntry(exec, abort_group)
                    )
                else:
                    run_state.non_hermetic_test_queue.put_nowait(
                        ExecEntry(exec, abort_group)
                    )

        tasks = []

        abort_all_tests_event = asyncio.Event()
        test_failure_observed: bool = False

        async def test_cancellation_handler() -> None:
            await self._end_execution_request_event.wait()
            recorder.emit_warning_message("Received request to terminate...")
            abort_all_tests_event.set()

        test_cancellation_handler_task = asyncio.Task(
            test_cancellation_handler()
        )

        maybe_debugger: subprocess.Popen[bytes] | None = None
        debugger_ready: asyncio.Condition = asyncio.Condition()

        if flags.debugger_should_spawn():

            async def on_debugger_ready() -> None:
                # TODO(b/329317913): Emit a debugger event here.
                async with debugger_ready:
                    debugger_ready.notify_all()

            maybe_debugger = debugger.spawn(
                tests.selected,
                on_debugger_ready,
                recorder=recorder,
                break_on_failure=flags.break_on_failure,
                enable_debug_adapter=flags.enable_debug_adapter,
                debug_adapter_port=flags.debug_adapter_port,
                breakpoints=flags.breakpoints,
            )

        # If we're in any debugging context, disable timeouts.
        if flags.debugger_will_attach():
            if (flags.timeout is None) or (
                flags.timeout and flags.timeout != 0
            ):
                recorder.emit_warning_message(
                    "Disabling timeouts during debugging session."
                )
                flags.timeout = 0

        async def test_executor() -> None:
            nonlocal test_failure_observed
            to_run: ExecEntry
            was_non_hermetic: bool = False

            while True:
                async with run_condition:
                    # Wait until we are allowed to try to run a test.
                    while run_state.total_running == max_parallel:
                        await run_condition.wait()

                    # If we should not execute any more tests, quit.
                    if abort_all_tests_event.is_set():
                        return

                    if (
                        run_state.non_hermetic_running == 0
                        and not run_state.non_hermetic_test_queue.empty()
                    ):
                        to_run = run_state.non_hermetic_test_queue.get_nowait()
                        run_state.non_hermetic_running += 1
                        was_non_hermetic = True
                    elif run_state.hermetic_test_queue.empty():
                        return
                    else:
                        to_run = run_state.hermetic_test_queue.get_nowait()
                        was_non_hermetic = False
                    run_state.total_running += 1

                test_suite_id = recorder.emit_test_suite_started(
                    to_run.exec.name(), not was_non_hermetic, parent=test_group
                )
                status: event.TestSuiteStatus = (
                    event.TestSuiteStatus.FAILED_TO_START
                )
                message: str | None = None
                try:
                    if not to_run.abort_group.is_set():
                        # Only run if this group was not already aborted.
                        command_line = " ".join(to_run.exec.command_line())
                        recorder.emit_instruction_message(
                            f"Command: {command_line}"
                        )

                        # Wait for the command completion and any other signal that
                        # means we should stop running the test.
                        run_task = asyncio.create_task(
                            to_run.exec.run(
                                recorder,
                                flags,
                                test_suite_id,
                                timeout=flags.timeout,
                                abort_signal=abort_all_tests_event,
                            )
                        )
                        abort_task = asyncio.create_task(
                            to_run.abort_group.wait()
                        )
                        done, pending = await asyncio.wait(
                            [run_task, abort_task],
                            return_when=asyncio.FIRST_COMPLETED,
                        )
                        for r in pending:
                            # Cancel pending tasks.
                            # This must happen before we throw exceptions to ensure
                            # tasks are properly cleaned up.
                            r.cancel()
                        if pending:
                            # Propagate cancellations
                            await asyncio.wait(pending)

                        command_output = None
                        for r in done:
                            # Re-throw exceptions.
                            exc = r.exception()
                            if exc:
                                if hasattr(exc, "command_output"):
                                    command_output = getattr(
                                        exc, "command_output", None
                                    )
                                raise exc
                            res = r.result()
                            if r is run_task:
                                command_output = res

                    if abort_all_tests_event.is_set():
                        status = event.TestSuiteStatus.ABORTED
                        message = "Test suite aborted due to another failure"
                    elif to_run.abort_group.is_set():
                        status = event.TestSuiteStatus.ABORTED
                        message = "Aborted re-runs due to another failure"
                    else:
                        status = event.TestSuiteStatus.PASSED
                except execution.TestCouldNotRun as e:
                    status = event.TestSuiteStatus.FAILED_TO_START
                    message = str(e)
                    test_failure_observed = True
                    to_run.abort_group.set()
                    if flags.fail:
                        abort_all_tests_event.set()
                except execution.TestSkipped as e:
                    status = event.TestSuiteStatus.SKIPPED
                    message = str(e)
                except (execution.TestTimeout, execution.TestFailed) as e:
                    if isinstance(e, execution.TestTimeout):
                        status = event.TestSuiteStatus.TIMEOUT
                        test_failure_observed = True
                    elif self._end_execution_request_event.is_set():
                        # Terminating the tests will end up here, since they will have a
                        # non-zero exit code following SIGTERM.
                        status = event.TestSuiteStatus.ABORTED
                        message = "Test suite aborted due to user interrupt."
                    else:
                        status = event.TestSuiteStatus.FAILED
                        test_failure_observed = True

                    # Abort other tests in this group.
                    to_run.abort_group.set()

                    if flags.fail:
                        # Abort all other running tests, dropping through to the
                        # following run state code to trigger any waiting executors.
                        abort_all_tests_event.set()
                finally:
                    recorder.emit_test_suite_ended(
                        test_suite_id, status, message
                    )
                    test_type = (
                        "host"
                        if not to_run.exec._test.needs_device()
                        else "device"
                    )
                    # CommandOutput might not exist if test failed to start or was skipped
                    exit_code = (
                        command_output.return_code if command_output else None
                    )

                    stdout_log_path = None
                    stderr_log_path = None
                    if self._agent_log_dir and command_output:
                        import re

                        safe_name = re.sub(
                            r"[^A-Za-z0-9_\.]", "_", to_run.exec.name()
                        )
                        stdout_log_path = (
                            f"{self._agent_log_dir}/{safe_name}.stdout.log"
                        )
                        stderr_log_path = (
                            f"{self._agent_log_dir}/{safe_name}.stderr.log"
                        )
                        if self._agent_log_dir:
                            os.makedirs(self._agent_log_dir, exist_ok=True)
                        with open(stdout_log_path, "w") as f:
                            f.write(getattr(command_output, "stdout", ""))
                        with open(stderr_log_path, "w") as f:
                            f.write(getattr(command_output, "stderr", ""))

                    self._summary.add_test(
                        summary.TestResult(
                            name=to_run.exec.name(),
                            type=test_type,
                            outcome=status.value,
                            exit_code=exit_code,
                            log_path=None,
                            stdout_log_path=stdout_log_path,
                            stderr_log_path=stderr_log_path,
                            duration_seconds=None,
                            message=message,
                        )
                    )

                async with run_condition:
                    run_state.total_running -= 1
                    if was_non_hermetic:
                        run_state.non_hermetic_running -= 1
                    run_condition.notify()

        # Wait for the debugger to signal that it is ready.
        if maybe_debugger is not None:
            async with debugger_ready:
                await debugger_ready.wait()

        for _ in range(max_parallel):
            tasks.append(asyncio.create_task(test_executor()))

        await asyncio.wait(tasks)

        if maybe_debugger is not None:
            # Close the fifo to signal zxdb to close and reset stdout to /dev/null so termout doesn't fail its cleanup.
            sys.stdout.close()
            sys.stdout = open(os.devnull, "w")

            # This is a synchronous wait and we don't want to block the event loop, so run it in the
            # default thread executor.
            loop = asyncio.get_event_loop()
            await loop.run_in_executor(None, maybe_debugger.wait)

        recorder.emit_end(id=test_group)

        test_cancellation_handler_task.cancel()

        return (
            not test_failure_observed
            and not self._end_execution_request_event.is_set()
        )

    def _list_runtime_deps(
        self,
        tests: selection_types.TestSelections,
    ) -> None:
        flags = self._flags
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None
        for t in tests.selected:
            recorder.emit_verbatim_message(
                statusinfo.green_highlight(f"{t.name()}:", style=flags.style)
            )
            runtime_deps = t.build.test.runtime_deps
            if runtime_deps is not None:
                runtime_deps_path = os.path.join(exec_env.out_dir, runtime_deps)
                with open(runtime_deps_path, "r") as deps_file:
                    deps: list[str] = json.load(deps_file)
                    recorder.emit_instruction_message(
                        f"  Runtime deps file at: {runtime_deps_path}"
                    )
                    if len(deps) > 0:
                        for dep in deps:
                            recorder.emit_verbatim_message(f"  {dep}")
                    else:
                        recorder.emit_instruction_message(f"  File is empty")
            else:
                recorder.emit_verbatim_message(
                    "  No runtime deps found for this test"
                )

    async def _enumerate_test_cases(
        self,
        tests: selection_types.TestSelections,
    ) -> int:
        """Lists the test cases selected by `tests`.

        Args:
            tests (selection.TestSelections): The selected tests to list cases.

        Returns:
            int: 1 if any tests failed to enumerate. 0 otherwise.
        """
        flags = self._flags
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None

        # Get the set of test executions that support enumeration.
        executions = [
            e
            for t in tests.selected
            if (
                e := execution.TestExecution(t, exec_env, flags)
            ).enumerate_cases_command_line()
            is not None
        ]

        wont_enumerate_count = len(tests.selected) - len(executions)

        outputs: list[command.CommandOutput | None] = []
        # Limit parallelism
        sem = asyncio.Semaphore(8)
        group_id = recorder.emit_event_group(
            "Enumerate test cases", queued_events=len(executions)
        )

        async def run_one_enumeration(
            e: execution.TestExecution,
        ) -> command.CommandOutput | None:
            if not e.enumerate_cases_command_line():
                return None

            async with sem:
                return await e.enumerate_test_cases(
                    recorder=recorder, parent=group_id
                )

        tasks = [run_one_enumeration(e) for e in executions]
        if tasks:
            outputs = list(await asyncio.gather(*tasks))

        recorder.emit_end(id=group_id)

        assert len(outputs) == len(executions)

        if wont_enumerate_count > 0:
            recorder.emit_info_message(
                f"\n{wont_enumerate_count:d} tests do not support enumeration"
            )

        failed_enumeration_names = []
        for output, exec in zip(outputs, executions):
            if output is None or output.return_code != 0:
                failed_enumeration_names.append(exec.name())
                continue

            # We use a template to format the output because different test types (e.g. host vs device)
            # require different commands to run per test case.
            # - Host tests (like Mobly) are executed directly.
            # - Device tests are typically run via `ffx test run`.
            command_template = exec.enumerate_cases_output_template()
            recorder.emit_enumerate_test_cases(
                exec.name(),
                list(output.stdout.splitlines()),
                command_template=command_template,
            )

        if failed_enumeration_names:
            recorder.emit_info_message(
                f"{len(failed_enumeration_names)} tests could not be enumerated"
            )
            return 1
        return 0

    def _start_package_server(
        self,
    ) -> tuple[asyncio.Task[typing.Any], asyncio.Event]:
        """
        Start a temporary package server.

        Returns:
            tuple[Task, Event]: A tuple of the asynchronous task and the event used to cancel it.
        """
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None

        cancel_event = asyncio.Event()

        # Aborting the `fx serve` task stops the repository server but does not remove the
        # corresponding repository config or rewrite rule from the device. The repository config is
        # explicitly deregistered from the device (which also removes the rewrite rule) before
        # cancelling the server task to avoid the pkg-resolver's AutoClient logging that the server
        # has gone away.
        async def impl() -> None:
            repo_name = f"fxtest-temp-{uuid.uuid4()}"
            repo_deregistered_event = asyncio.Event()
            serve_task = asyncio.create_task(
                execution.run_command(
                    *exec_env.fx_cmd_line(
                        "serve",
                        "-l",
                        "0",
                        "--name",
                        repo_name,
                    ),
                    recorder=recorder,
                    abort_signal=repo_deregistered_event,
                    quiet_mode=True,
                )
            )
            try:
                await cancel_event.wait()
            finally:
                try:
                    deregister_output = await execution.run_command(
                        *exec_env.fx_cmd_line(
                            "ffx",
                            "target",
                            "repository",
                            "deregister",
                            "-r",
                            repo_name,
                        ),
                        recorder=recorder,
                        quiet_mode=True,
                        timeout=5.0,
                    )
                    if (
                        deregister_output is None
                        or deregister_output.return_code != 0
                    ):
                        raise RuntimeError(
                            f"exit code {deregister_output.return_code if deregister_output is not None else -1}"
                        )
                except Exception as e:
                    recorder.emit_warning_message(
                        f"Failed to deregister temporary package repository {repo_name}: {e}"
                    )
                finally:
                    repo_deregistered_event.set()
                    try:
                        await asyncio.wait_for(serve_task, timeout=10.0)
                    except TimeoutError:
                        recorder.emit_warning_message(
                            f"Timed out waiting for temporary package server ({repo_name}) to stop."
                        )
                        serve_task.cancel()

        task = asyncio.create_task(impl())
        self._package_server_task = task
        self._package_server_event = cancel_event
        return (task, cancel_event)

    async def _get_active_devices(self) -> list[dict[str, typing.Any]]:
        """Fetch the list of active devices from ffx.

        Returns:
            list[dict[str, Any]]: A list of active devices.
        """
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None

        output = await execution.run_command(
            *exec_env.fx_cmd_line("ffx", "--machine", "json", "target", "list"),
            recorder=recorder,
            quiet_mode=True,
            timeout=_DEFAULT_PROBE_TIMEOUT_SECONDS,
        )
        if output is None or output.return_code != 0:
            return []

        try:
            targets = json.loads(output.stdout)
            if isinstance(targets, list):
                return [
                    t
                    for t in targets
                    if isinstance(t, dict) and t.get("rcs_state") == "Y"
                ]
        except (json.JSONDecodeError, TypeError) as e:
            recorder.emit_warning_message(
                f"Failed to parse target list JSON: {e}"
            )

        return []

    async def _bind_to_active_device(self) -> None:
        """If exactly one active device is found and FUCHSIA_NODENAME is not set,
        set FUCHSIA_NODENAME to target this device.
        """
        recorder = self._recorder
        active_devices = await self._get_active_devices()
        if len(active_devices) == 1:
            nodename = active_devices[0].get("nodename")
            if nodename:
                _set_target_nodename(nodename)
                recorder.emit_info_message(
                    f"Found exactly one active device: {nodename}. "
                    "Setting FUCHSIA_NODENAME to target this device."
                )

    async def _has_active_device(self) -> bool:
        """Check if any active devices are reachable.

        Returns:
            bool: True if an active device is found, False otherwise.
        """
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None

        # Check if a specific target is requested via FUCHSIA_NODENAME (set by fx -t).
        target = os.environ.get("FUCHSIA_NODENAME")

        # If not, check if a default target is configured.
        if not target:
            default_output = await execution.run_command(
                *exec_env.fx_cmd_line("ffx", "target", "default", "get"),
                recorder=recorder,
                quiet_mode=True,
                timeout=_DEFAULT_PROBE_TIMEOUT_SECONDS,
            )
            if default_output and default_output.return_code == 0:
                target = default_output.stdout.strip()

        # If a target is identified, verify its reachability.
        if target:
            echo_output = await execution.run_command(
                *exec_env.fx_cmd_line("ffx", "-t", target, "target", "echo"),
                recorder=recorder,
                quiet_mode=True,
                timeout=_DEFAULT_PROBE_TIMEOUT_SECONDS,
            )
            if echo_output and echo_output.return_code == 0:
                return True

        # Fall back to checking all discovered devices.
        active_devices = await self._get_active_devices()
        return len(active_devices) > 0

    async def _wait_for_repository_registration(self) -> bool:
        """Wait for repository to be registered."""
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None
        for _ in range(30):
            try:
                if await is_fuchsia_repo_registered_on_device(
                    exec_env, recorder
                ):
                    return True
            except Exception:
                pass
            await asyncio.sleep(1)
        return False

    async def _resolve_target_ip(
        self, nodename: str, ffx_config: tuple[str, ...] = ()
    ) -> str | None:
        """Resolve the target IP address and SSH port for a given nodename.

        Args:
            nodename (str): The nodename of the target to search for.
            ffx_config (tuple[str, ...], optional): Config arguments for ffx. Defaults to ().

        Returns:
            str | None: The resolved target address string, or None if not found.
        """
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None

        target_list_output = await execution.run_command(
            *exec_env.fx_cmd_line(
                "ffx",
                "--machine",
                "json",
                *ffx_config,
                "target",
                "list",
            ),
            recorder=recorder,
            quiet_mode=True,
            timeout=_DEFAULT_PROBE_TIMEOUT_SECONDS,
        )
        if not target_list_output or target_list_output.return_code != 0:
            return None

        try:
            targets = json.loads(target_list_output.stdout)
            for t in targets:
                if t.get("nodename") == nodename:
                    for addr in t.get("addresses", []):
                        ip = addr.get("ip")
                        port = addr.get("ssh_port")
                        if not ip or port is None:
                            recorder.emit_warning_message(
                                f"Invalid address for {nodename}: {addr}"
                            )
                            continue
                        if port == 0:
                            return ip
                        return f"[{ip}]:{port}" if ":" in ip else f"{ip}:{port}"
        except json.JSONDecodeError:
            pass
        return None

    async def _get_emu_start_timeout(
        self, config_args: tuple[str, ...]
    ) -> float:
        """Query configured emulator start timeout from ffx config.

        Args:
            config_args (tuple[str, ...]): Config arguments for ffx.

        Returns:
            float: The configured timeout in seconds, or the default timeout if not configured or invalid.
        """
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None

        output = await execution.run_command(
            *exec_env.fx_cmd_line(
                "ffx",
                *config_args,
                "config",
                "get",
                "emu.start.timeout",
            ),
            recorder=recorder,
            timeout=_DEFAULT_PROBE_TIMEOUT_SECONDS,
        )
        if output is not None and output.return_code == 0:
            try:
                return float(output.stdout.strip().strip('"'))
            except ValueError:
                pass

        return _DEFAULT_EMU_START_TIMEOUT_SECONDS

    async def _start_emulator(self) -> bool:
        """Start a headless emulator.

        Note: As a side effect, this exports os.environ["FUCHSIA_NODENAME"] to the
        discovered emulator IP address and SSH port so downstream commands target it.

        Returns:
            bool: True if the emulator starts successfully, False otherwise.
        """
        recorder = self._recorder
        exec_env = self._exec_env
        assert exec_env is not None

        # Configure `ffx emu` to use an anonymous instance directory and --net user so that
        # emulator instances are private and independent of other `fx test` invocations.
        self._emu_instance_dir = tempfile.TemporaryDirectory(
            prefix="fxtest-emu-"
        )
        config_args = (
            "--config",
            f"emu.instance_dir={self._emu_instance_dir.name}",
        )

        emu_name = os.path.basename(self._emu_instance_dir.name)
        recorder.emit_instruction_message(
            "\nNo active device detected. Starting a headless emulator..."
        )

        # Query configured emulator start timeout from ffx config (defaulting to 60.0s)
        # and allow padding for start command execution.
        emu_timeout = await self._get_emu_start_timeout(config_args)

        start_timeout = emu_timeout + _EMU_START_TIMEOUT_PADDING_SECONDS
        output = await execution.run_command(
            *exec_env.fx_cmd_line(
                "ffx",
                *config_args,
                "emu",
                "start",
                "--headless",
                "--net",
                "user",
                "--name",
                emu_name,
            ),
            recorder=recorder,
            timeout=start_timeout,
        )
        if output is None or output.return_code != 0:
            recorder.emit_warning_message("Failed to start emulator.")
            return False

        # Register atexit teardown hook as a fallback to prevent
        # orphaned/zombie emulators on asyncio.CancelledError from SIGINT, etc.
        atexit.register(self._fallback_stop_emulator)

        # Wait for the emulator to be ready.
        recorder.emit_instruction_message("Waiting for emulator to be ready...")
        wait_output = await execution.run_command(
            *exec_env.fx_cmd_line(
                "ffx",
                *config_args,
                "--target",
                emu_name,
                "target",
                "wait",
                "-t",
                str(int(_TARGET_WAIT_TIMEOUT_SECONDS)),
            ),
            recorder=recorder,
            timeout=_TARGET_WAIT_TIMEOUT_SECONDS,
        )
        if wait_output is None or wait_output.return_code != 0:
            recorder.emit_warning_message(
                "Emulator failed to become ready in time."
            )
            await self._teardown_emulator()
            return False

        # Resolve the emulator's IP address and port.
        emu_addr = await self._resolve_target_ip(emu_name, config_args)
        if not emu_addr:
            recorder.emit_warning_message(
                "Failed to resolve temporary emulator IP address."
            )
            await self._teardown_emulator()
            return False

        recorder.emit_instruction_message(f"Emulator ready at {emu_addr}")
        _set_target_nodename(emu_addr)
        return True

    def _get_emu_stop_cmd(self) -> list[str]:
        assert self._exec_env is not None
        assert self._emu_instance_dir is not None
        return self._exec_env.fx_cmd_line(
            "ffx",
            "--config",
            f"emu.instance_dir={self._emu_instance_dir.name}",
            "emu",
            "stop",
            os.path.basename(self._emu_instance_dir.name),
        )

    async def _teardown_emulator(self) -> bool:
        """Stop the headless emulator and cleanup its temporary directory.

        Returns:
            bool: True if the emulator stops successfully, False otherwise.
        """
        if self._emu_instance_dir is None:
            return True

        recorder = self._recorder
        recorder.emit_instruction_message("\nStopping the headless emulator...")
        try:
            output = await execution.run_command(
                *self._get_emu_stop_cmd(),
                recorder=recorder,
                timeout=_DEFAULT_PROBE_TIMEOUT_SECONDS,
            )
            if output is None or output.return_code != 0:
                recorder.emit_warning_message("Failed to stop emulator.")
                return False
            return True
        finally:
            atexit.unregister(self._fallback_stop_emulator)
            try:
                self._emu_instance_dir.cleanup()
            except Exception as e:
                recorder.emit_warning_message(
                    "Failed to clean up temporary emulator instance directory at "
                    f"{self._emu_instance_dir.name}: {e}"
                )
            self._emu_instance_dir = None
            _set_target_nodename(None)

    def _fallback_stop_emulator(self) -> None:
        """Stops the headless emulator synchronously, suitable for atexit."""
        if self._emu_instance_dir is None or self._exec_env is None:
            return

        try:
            subprocess.run(
                self._get_emu_stop_cmd(),
                check=False,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=15,
            )
        except Exception:
            pass
        finally:
            if self._emu_instance_dir is not None:
                try:
                    self._emu_instance_dir.cleanup()
                except Exception:
                    pass
                self._emu_instance_dir = None
            _set_target_nodename(None)


async def has_package_server_connected_to_device(
    exec_env: environment.ExecutionEnvironment,
    recorder: event.EventRecorder,
    parent: event.Id | None = None,
    timeout: float = _PACKAGE_SERVER_PROBE_TIMEOUT_SECONDS,
) -> bool:
    """Check if a package server is running and registered to the device.

    Args:
        exec_env (environment.ExecutionEnvironment): Execution environment.
        recorder (event.EventRecorder): Recorder for events.
        parent (event.Id, optional): Parent task ID. Defaults to None.
        timeout (float, optional): Subprocess timeout in seconds.

    Returns:
        bool: True only if a package server is running and registered to the device.
    """
    output = await execution.run_command(
        *exec_env.fx_cmd_line(
            "is-package-server-running",
        ),
        recorder=recorder,
        parent=parent,
        timeout=timeout,
    )
    if output is None or output.return_code != 0:
        return False

    return await is_fuchsia_repo_registered_on_device(
        exec_env, recorder, parent=parent, timeout=timeout
    )


async def is_fuchsia_repo_registered_on_device(
    exec_env: environment.ExecutionEnvironment,
    recorder: event.EventRecorder,
    parent: event.Id | None = None,
    timeout: float = _PACKAGE_SERVER_PROBE_TIMEOUT_SECONDS,
) -> bool:
    """Check whether fuchsia.com repository is registered on the device."""
    repo_output = await execution.run_command(
        *exec_env.fx_cmd_line(
            "ffx",
            "--machine",
            "json",
            "target",
            "repository",
            "list",
        ),
        recorder=recorder,
        parent=parent,
        timeout=timeout,
        quiet_mode=True,
    )
    return (
        repo_output is not None
        and repo_output.return_code == 0
        and _is_fuchsia_repo_registered(repo_output.stdout)
    )


def _is_fuchsia_repo_registered(output: str) -> bool:
    """Check whether fuchsia.com repository is registered in ffx output."""
    parsed = json.loads(output)
    # Expected format from ffx --machine json: {"ok": {"data": [...]}}
    entries = []
    if isinstance(parsed, dict) and "ok" in parsed:
        data = parsed["ok"].get("data", [])
        if isinstance(data, list):
            entries = data
    elif isinstance(parsed, list):
        entries = parsed
    for entry in entries:
        if isinstance(entry, dict):
            name = entry.get("name", "")
            aliases = entry.get("aliases", [])
            if (
                name == "fuchsia.com"
                or name == "fuchsia-pkg://fuchsia.com"
                or "fuchsia.com" in aliases
            ):
                return True
    return False


def _emit_build_failure(
    recorder: event.EventRecorder,
    output: command.CommandOutput | None,
) -> str:
    """Emit compiler diagnostics from a failed build and format error message."""
    if output is not None:
        msg = "\n".join(filter(None, [output.stdout, output.stderr]))
        if msg:
            recorder.emit_verbatim_message(msg)
    rc = output.return_code if output is not None else -1
    return f"Build returned non-zero exit code {rc}"


async def run_build(
    exec_env: environment.ExecutionEnvironment,
    build_command_line: list[str],
    recorder: event.EventRecorder | None = None,
    parent: event.Id | None = None,
    abort_signal: asyncio.Event | None = None,
    print_verbatim: bool = False,
) -> command.CommandOutput | None:
    return await execution.run_command(
        *exec_env.fx_cmd_line("build", *build_command_line),
        recorder=recorder,
        parent=parent,
        abort_signal=abort_signal,
        print_verbatim=print_verbatim,
        quiet_mode=True,
    )


async def run_commands_in_parallel(
    commands: list[list[str]],
    group_name: str,
    recorder: event.EventRecorder | None = None,
    maximum_parallel: int | None = None,
) -> list[command.CommandOutput | None]:
    assert recorder

    parent = recorder.emit_event_group(group_name, queued_events=len(commands))
    output: list[command.CommandOutput | None] = [None] * len(commands)
    in_progress: typing.Set[asyncio.Task[None]] = set()

    index = 0

    def can_add() -> bool:
        nonlocal index
        return index < len(commands) and (
            maximum_parallel is None or len(in_progress) < maximum_parallel
        )

    while index < len(commands) or in_progress:
        while can_add():

            async def set_index(i: int) -> None:
                output[i] = await execution.run_command(
                    *commands[i], recorder=recorder, parent=parent
                )

            in_progress.add(asyncio.create_task(set_index(index)))
            index += 1

        _, in_progress = await asyncio.wait(
            in_progress, return_when="FIRST_COMPLETED"
        )

    recorder.emit_end(id=parent)

    return output


if __name__ == "__main__":
    main()
