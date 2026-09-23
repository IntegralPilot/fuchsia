# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import logging
import os
from time import monotonic
from typing import Any, List

import fuchsia_base_test
from mobly import test_runner
from trace_processing import trace_importing, trace_model

# TODO(b/396700496) Make this support parallel test-runs
# (or verify that it already does)

_LOGGER = logging.getLogger(__name__)

# This is a Lacewing-style test that wraps a hermetic device test,
# snoops its FIDL activity, and writes that activity to files in
# the test output directory.


class FidlReaderTest(fuchsia_base_test.FuchsiaBaseTest):
    async def test_integration_testcase(self) -> None:
        data_writer = JsonWriter(self.log_path)
        device_test_url = self.user_params.get(
            "device_test_url", "URL Not Found"
        )
        host_output_path = self.test_case_path
        test_realm_name = self.user_params.get(
            "device_test_realm", "/core/testing/ctf-tests"
        )
        min_severity = self.user_params.get("min_severity", None)
        max_severity = self.user_params.get("max_severity", None)
        progress = ProgressRecorder(
            writer=data_writer,
            url=device_test_url,
            realm=test_realm_name,
            log_severity=(min_severity, max_severity),
        )
        async with self.dut.tracing.trace_session(
            categories=[
                "kernel:meta",
                "kernel:ipc",
                "component:start",
            ],
            buffer_size=72,
            download=True,
            directory=host_output_path,
            trace_file="trace.fxt",
        ):
            progress.record_milestone("started tracing")
            ffx_test_args = [
                "--realm",
                test_realm_name,
            ]
            if min_severity:
                ffx_test_args += ["--min-severity-logs", min_severity]
            if max_severity:
                ffx_test_args += ["--max-severity-logs", max_severity]
            self.dut.ffx.run_test_component(
                device_test_url,
                # TODO(b/396759445): Audit these capabilities per-test.
                capture_output=False,
                ffx_test_args=ffx_test_args,
            )
            progress.record_milestone("ran the test")
        progress.record_milestone("finished tracing")

        # $ ffx trace symbolize --fxt trace.fxt --outfile trace.fxt
        infile = os.path.join(host_output_path, "trace.fxt")
        outfile = os.path.join(host_output_path, "trace.fxt")
        self.dut.ffx.run(
            ["trace", "symbolize", "--fxt", infile, "--outfile", outfile]
        )
        progress.record_milestone("symbolized the trace")

        json_trace_file: str = trace_importing.convert_trace_file_to_json(
            os.path.join(host_output_path, "trace.fxt")
        )
        progress.record_milestone("converted the trace")

        model: trace_model.Model = trace_importing.create_model_from_file_path(
            json_trace_file
        )
        progress.record_milestone("created the model")

        scanned_model = EventScanner(model, device_test_url)
        progress.record_milestone("scanned the model")

        data_writer.write(
            "intra_calls.freeform.json", scanned_model.intra_calls()
        )
        data_writer.write(
            "incoming_calls.freeform.json", scanned_model.incoming_calls()
        )
        data_writer.write(
            "outgoing_calls.freeform.json", scanned_model.outgoing_calls()
        )
        progress.record_milestone("wrote the outputs")


class JsonWriter:
    def __init__(self, log_path: str):
        self.log_path = log_path

    def write(self, fname: str, data: Any) -> None:
        out_path = os.path.join(self.log_path, fname)
        with open(out_path, "w") as f:
            json.dump(data, f, indent=4, default=vars)


# Data-holder classes are used for outputting a clean JSON schema.


# Data-holder class for FIDL client or server processes,
# inside or outside the hermetic test.
class FidlEnd:
    def __init__(self) -> None:
        self.name = ""
        self.is_test = False


# Process outside the hermetic test. We may not know the URL or moniker.
class FidlExtern(FidlEnd):
    def __init__(
        self, pid: int, url: str | None = None, moniker: str | None = None
    ) -> None:
        super().__init__()
        self.pid = pid
        if url != None:
            self.url = url
        if moniker != None:
            self.moniker = moniker


# Process inside the hermetic test.
class FidlTest(FidlEnd):
    def __init__(self, pid: int, url: str, moniker: str) -> None:
        super().__init__()
        self.pid = pid
        self.url = url
        self.moniker = moniker
        self.is_test = True


# Data holder class containing client and server processes, and timestamps of calls
# from client to server. Two-way FIDL calls will show up as both client->server
# and server->client.
class FidlChannel:
    def __init__(self, sender: FidlEnd, receiver: FidlEnd) -> None:
        self.sender = sender
        self.receiver = receiver

    def add_timestamp(self, timestamp: int) -> None:
        if not hasattr(self, "timestamps"):
            self.timestamps = []
        self.timestamps.append(timestamp)


# Data holder class for outputting progress data. Used by ProgressRecorder.
class ProgressData:
    def __init__(
        self, url: str, realm: str, log_severity: tuple[str | None, str | None]
    ):
        self.log_severity = log_severity
        self.realm = realm
        self.url = url
        self.timestamps: List[tuple[str, float]] = []


# Progress recorder for updating the progress output file.
class ProgressRecorder:
    def __init__(
        self,
        writer: JsonWriter,
        url: str,
        realm: str,
        log_severity: tuple[str | None, str | None],
    ):
        self.writer = writer
        self.progress = ProgressData(
            url=url, realm=realm, log_severity=log_severity
        )
        self.write_file()
        self.last_timestamp = monotonic()

    def record_milestone(self, milestone_name: str) -> None:
        now = monotonic()
        milestone = (milestone_name, now - self.last_timestamp)
        _LOGGER.info(f"Recording milestone {milestone}")
        self.progress.timestamps.append(milestone)
        self.write_file()
        self.last_timestamp = now

    def write_file(self) -> None:
        self.writer.write("fidl_progress.freeform.json", self.progress)


# This class stores transaction data for a single FIDL method.
# It's intended to be a dictionary value, so it doesn't store the name.
class FidlMethod:
    def __init__(self) -> None:
        # Channel info for each (sender, receiver) pair.
        self.channels: dict[tuple[FidlEnd, FidlEnd], FidlChannel] = {}

    def add_call(
        self, sender: FidlEnd, receiver: FidlEnd, timestamp: int | None = None
    ) -> None:
        endpoints_key = (sender, receiver)
        if endpoints_key not in self.channels:
            self.channels[endpoints_key] = FidlChannel(sender, receiver)
        if timestamp:
            self.channels[endpoints_key].add_timestamp(timestamp)

    def channels_for_scope(
        self, sender_is_test: bool, receiver_is_test: bool
    ) -> list[FidlChannel]:
        result = []
        for sender, receiver in self.channels:
            if (
                sender
                and sender.is_test == sender_is_test
                and receiver
                and receiver.is_test == receiver_is_test
            ):
                endpoints_key = (sender, receiver)
                result.append(self.channels[endpoints_key])
        return result


# This class extracts and stores data on FIDL transactions from Duration events.
class FidlTransactions:
    def __init__(self, pid_lookup: dict[int, FidlEnd]):
        # Translates from pid to FidlEnd.
        self.pid_lookup = pid_lookup

        # Stores info about each call.
        self.call_info: dict[str, FidlMethod] = {}

        # Every FIDL transaction creates two durations, one on the receiving end,
        # one on the sending end. Both durations point to both flow events.
        # For transactions between two test components, we'll
        # see both of those, and we only want to record one. So, if we get a duration
        # where we've seen the sender event, we should ignore it.
        self.seen_sender_events: set[trace_model.FlowEvent] = set()

        # Some durations have empty child flows. Summarize rather than spam the log.
        self.empty_child_flows: int = 0

    def log_anomalies(self) -> None:
        _LOGGER.info(
            f"Found {self.empty_child_flows} empty duration.child_flows"
        )

    def process_duration(self, duration: trace_model.DurationEvent) -> None:
        if len(duration.child_flows) == 0:
            self.empty_child_flows += 1
            return
        if len(duration.child_flows) != 1:
            _LOGGER.warning(
                f"Unexpected event, wrong child_flows: {duration.child_flows}"
            )
            return
        flow = duration.child_flows[0]
        if "ordinal" not in duration.args:
            # We see a few of these in the trace. Ignore them.
            return
        if "method" not in duration.args:
            method_name = duration.args["ordinal"]
        else:
            method_name = duration.args["method"]
        (sender, receiver) = self.extract_flows(flow)
        if not sender or not receiver:
            return
        if sender in self.seen_sender_events:
            # We saw the other half of this already.
            return
        self.seen_sender_events.add(sender)
        if method_name not in self.call_info:
            self.call_info[method_name] = FidlMethod()
        self.call_info[method_name].add_call(
            self.pid_lookup[sender.pid], self.pid_lookup[receiver.pid]
        )

    # Extract the sender and receiver of the message.
    def extract_flows(
        self, flow: trace_model.FlowEvent
    ) -> tuple[trace_model.FlowEvent | None, trace_model.FlowEvent | None]:
        # We expect each flow to be either the first or second of a pair. If
        # it's part of >2 flows, swallow the error and return (None, None).
        if flow.next_flow:
            if flow.previous_flow or flow.next_flow.next_flow:
                return None, None
            return flow, flow.next_flow
        if flow.previous_flow:
            if flow.previous_flow.previous_flow:
                return None, None
            return flow.previous_flow, flow
        # We typically see a couple of durations without flows. Probably Archivist was
        # in the middle of something when the tracing stopped, and the message
        # didn't get fully recorded.
        return None, None

    def fidl_calls(
        self, sender_is_test: bool, receiver_is_test: bool
    ) -> dict[str, list[FidlChannel]]:
        output: dict[str, list[FidlChannel]] = {}
        for method_name in self.call_info:
            channels = self.call_info[method_name].channels_for_scope(
                sender_is_test, receiver_is_test
            )
            if channels:
                output[method_name] = channels
        return output


# Scans a trace model and gathers info on FIDL calls. intra_calls(), incoming_calls(),
# and outgoing_calls() will return dict[message_string, FidlChannel]
class EventScanner:
    def __init__(self, model: trace_model.Model, test_url: str):
        self.model = model
        self.test_url = test_url
        self.start_events = [
            event
            for event in self.model.all_events()
            if event.category == "component:start"
        ]
        self.realm_prefixes = self.find_test_realm_prefixes()
        self.fill_pid_lookup()
        self.transactions = FidlTransactions(self.pid_lookup)
        self.scan_for_fidl()

    def find_test_realm_prefixes(self) -> List[str]:
        test_runner_events = [
            event for event in self.start_events if "-test-" in event.name
        ]
        if not test_runner_events:
            _LOGGER.error(
                f"Expected at least one test-runner event for {self.test_url}, but found none"
            )
            return []
        test_monikers = [event.args["moniker"] for event in test_runner_events]
        # The part of the moniker before "test_wrapper/test" will be a prefix on all tests
        # started by this runner.
        realm_prefixes = [
            test_moniker.split("test_wrapper/test")[0]
            for test_moniker in test_monikers
        ]
        for prefix in realm_prefixes:
            _LOGGER.info(
                f"Found realm prefix {prefix} for test URL {self.test_url}"
            )
        return realm_prefixes

    def test_realm_prefix_for(self, moniker: str) -> str | None:
        for prefix in self.realm_prefixes:
            if moniker.startswith(prefix):
                return prefix
        return None

    def fill_pid_lookup(self) -> None:
        pid_lookup: dict[int, FidlEnd] = {}
        for event in self.start_events:
            moniker = event.args["moniker"]
            url = event.args["url"]
            pid = event.args["pid"]
            realm_prefix = self.test_realm_prefix_for(moniker)
            if realm_prefix:
                # It's inside the hermetic test realm
                pid_lookup[pid] = FidlTest(
                    pid=pid, url=url, moniker=moniker[len(realm_prefix) :]
                )
            else:
                # We got a start event so we know its strings, but it's extern to the hermetic test.
                pid_lookup[pid] = FidlExtern(pid=pid, url=url, moniker=moniker)
        for process in self.model.processes:
            # Fill in the pid_lookup dict with at least the process name.
            if process.pid not in pid_lookup:
                pid_lookup[process.pid] = FidlExtern(pid=process.pid)
            pid_lookup[process.pid].name = process.name
        self.pid_lookup = pid_lookup

    def scan_for_fidl(self) -> Any:
        self.calls: set[tuple[str, FidlEnd | None, FidlEnd | None]] = set()
        for process in self.model.processes:
            if process.pid not in self.pid_lookup:
                _LOGGER.warning(f"Pid not found in lookup dict: {process.pid}")
                continue
            if not self.pid_lookup[process.pid].is_test:
                # For messages between inside and outside the test, is_test will be true for at least one side.
                # We want to ignore messages where both sides are outside the test.
                # If the other side is inside, we'll see its Duration events.
                continue
            for thread in process.threads:
                for event in thread.events:
                    if event.category == "kernel:ipc" and isinstance(
                        event, trace_model.DurationEvent
                    ):
                        self.transactions.process_duration(event)
        self.transactions.log_anomalies()

    def intra_calls(self) -> dict[str, list[FidlChannel]]:
        return self.transactions.fidl_calls(True, True)

    def incoming_calls(self) -> dict[str, list[FidlChannel]]:
        return self.transactions.fidl_calls(False, True)

    def outgoing_calls(self) -> dict[str, list[FidlChannel]]:
        return self.transactions.fidl_calls(True, False)


if __name__ == "__main__":
    test_runner.main()
