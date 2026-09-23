# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Logic to deserialize a trace Model from JSON."""

import json
import logging
import math
import os
import pathlib
import stat
import subprocess
import types
from collections import defaultdict
from collections.abc import Set
from importlib.resources import as_file, files
from typing import Any, Dict, List, NamedTuple, Optional, Self, TextIO, Tuple

from tp_shell import PerfettoTraceProcessor
from trace_processing import data  # type: ignore[attr-defined]
from trace_processing import trace_model, trace_time

_LOGGER: logging.Logger = logging.getLogger("Performance")
_JSONLINES_SUFFIX = ".systemTraceEvents.jsonlines"


class _FlowKey:
    """A helper struct to group flow events."""

    def __init__(
        self, category: Optional[str], name: Optional[str], pid: int, id: str
    ) -> None:
        self.category: Optional[str] = category
        self.name: Optional[str] = name
        self.pid: int = pid  # Only used for 'local' flow ids.
        self.id: str = id

    @classmethod
    def from_trace_event(cls, trace_event: Dict[str, Any]) -> Self:
        category: Optional[str] = trace_event.get("cat")
        name: Optional[str] = trace_event.get("name")
        # _FlowKey is globally scoped unless specifically local.
        pid: int = 0

        # Helper to convert an object into a string.
        def as_string_id(obj: object) -> str:
            if isinstance(obj, str):
                return obj
            elif isinstance(obj, int):
                return str(obj)
            elif isinstance(obj, float):
                if math.isnan(obj):
                    raise TypeError("Got NaN double for id field value")
                elif obj % 1.0 != 0.0:
                    raise TypeError(
                        f"Got float with non-zero decimal place ({obj}) for id "
                        f"field value"
                    )
                else:
                    return str(int(obj))
            else:
                raise TypeError(
                    f"Got unexpected type {obj.__class__.__name__} for id "
                    f"field value: {obj}"
                )

        id = None
        if "id" in trace_event:
            id = as_string_id(trace_event["id"])
        elif "id2" in trace_event:
            id2 = trace_event["id2"]
            if "local" in id2:
                pid = trace_event["pid"]
                id = as_string_id(id2["local"])
            elif "global" in id2:
                id = as_string_id(id2["global"])
        if id is None:
            raise Exception(f"Could not find id in {trace_event}")

        return cls(category, name, pid, id)

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, _FlowKey):
            return False

        return (
            self.category == other.category
            and self.name == other.name
            and self.id == other.id
            and self.pid == other.pid
        )

    def __hash__(self) -> int:
        result = 17
        result = 37 * result + hash(self.category)
        result = 37 * result + hash(self.name)
        result = 37 * result + hash(self.id)
        result = 37 * result + hash(self.pid)
        return result


class _AsyncKey:
    """A helper struct to group async events."""

    def __init__(
        self, category: Optional[str], name: Optional[str], pid: int, id: int
    ) -> None:
        self.category: Optional[str] = category
        self.name: Optional[str] = name
        self.pid: int = pid
        self.id: int = id

    @classmethod
    def from_trace_event(cls, trace_event: Dict[str, Any]) -> Self:
        category: Optional[str] = trace_event.get("cat", None)
        name: Optional[str] = trace_event.get("name", None)
        pid: int = trace_event["pid"]

        # Helper to parse an object into an int, returning None if the object is
        # not parseable.
        def try_parse_int(s: str) -> Optional[int]:
            try:
                return int(s, 0)  # 0 base allows guessing hex, binary, etc
            except (TypeError, ValueError):
                return None

        id: Optional[int] = None
        if "id" in trace_event:
            if isinstance(trace_event["id"], int):
                id = trace_event["id"]
            elif isinstance(trace_event["id"], str):
                id = try_parse_int(trace_event["id"])
        elif "id2" in trace_event:
            id2 = trace_event["id2"]
            if "local" in id2:
                # 'local' id2 means scoped to the process.
                if isinstance(id2["local"], int):
                    id = id2["local"]
                elif isinstance(id2["local"], str):
                    id = try_parse_int(id2["local"])
            elif "global" in id2:
                pid = 0
                if isinstance(id2["global"], int):
                    id = id2["global"]
                elif isinstance(id2["global"], str):
                    id = try_parse_int(id2["global"])
        if id is None:
            raise Exception(f"Could not find id in {trace_event}")

        return cls(category, name, pid, id)

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, _AsyncKey):
            return False

        return (
            self.pid == other.pid
            and self.category == other.category
            and self.name == other.name
            and self.id == other.id
        )

    def __hash__(self) -> int:
        result = 17
        result = 37 * result + hash(self.pid)
        result = 37 * result + hash(self.category)
        result = 37 * result + hash(self.name)
        result = 37 * result + hash(self.id)
        return result


class PidMapping(NamedTuple):
    pid: int
    name: str


class TidMapping(NamedTuple):
    tid: int
    pid: int
    name: str


class ContextSwitchMapping(NamedTuple):
    cpu: int
    record: trace_model.ContextSwitch


class WakingMapping(NamedTuple):
    cpu: int
    record: trace_model.Waking


def convert_trace_file_to_json(
    trace_path: str | os.PathLike[Any],
    trace2json_path: str | os.PathLike[Any] | None = None,
    patterns: Set[str] | None = None,
    categories: Set[str] | None = None,
) -> str:
    """Converts the specified trace file to JSON.

    Args:
      trace_path: The path to the trace file to convert.
      trace2json_path: The path to the trace2json executable. When unset, find
          at a runtime_deps/trace2json location in a parent directory.
      patterns: Regexps to match against event names. Only events that match one or more of these
                patterns will be included in the converted trace file.
                Pass None to include all events.
                Pass the empty set to discard all events.
      categories: The names of categories to include. An event will be included if it either
                  matches one of the elements of `patterns`, OR it is in one of the categories
                  listed here.
                  Pass None or the empty set to defer entirely to `patterns`.

    Raises:
      subprocess.CalledProcessError: The trace2json process returned an error.

    Returns:
      The path to the converted trace file.
    """
    converted_path, _, _ = convert_trace_file_to_json_rusage(
        trace_path, trace2json_path, patterns, categories
    )
    return str(converted_path)


def time_convert_trace_file_to_json(
    trace_path: str | os.PathLike[Any],
    trace2json_path: str | os.PathLike[Any] | None = None,
    patterns: Set[str] | None = None,
    categories: Set[str] | None = None,
    split: bool = False,
    timer_cmd: list[str] | None = None,
) -> tuple[str, str, str]:
    """Converts the specified trace file to JSON.

    Args:
      trace_path: The path to the trace file to convert.
      trace2json_path: The path to the trace2json executable. When unset, find
          at a runtime_deps/trace2json location in a parent directory.
      patterns: Regexps to match against event names. Only events that match one or more of these
                patterns will be included in the converted trace file.
                Pass None to include all events.
                Pass the empty set to discard all events.
      categories: The names of categories to include. An event will be included if it either
                  matches one of the elements of `patterns`, OR it is in one of the categories
                  listed here.
                  Pass None or the empty set to defer entirely to `patterns`.
      split: Whether to split `systemTraceEvents` out into a separate, jsonlines-formatted, file.
      timer_cmd: Command (e.g. `/usr/bin/time -v`) to wrap conversion process in

    Raises:
      subprocess.CalledProcessError: The trace2json process returned an error.

    Returns:
      The path to the converted trace file.
      The path to the split-out jsonlines file, if requested, or the emoty string.
      Output of the conversion process.
    """
    (
        output_path,
        jsonlines_path,
        conversion_output,
    ) = convert_trace_file_to_json_rusage(
        trace_path, trace2json_path, patterns, categories, split
    )
    return (
        str(output_path),
        str(jsonlines_path) if jsonlines_path else "",
        conversion_output,
    )


def convert_trace_file_to_json_rusage(
    trace_path: str | os.PathLike[str],
    trace2json_path: str | os.PathLike[str] | None = None,
    patterns: Set[str] | None = None,
    categories: Set[str] | None = None,
    split: bool = False,
) -> tuple[os.PathLike[str], os.PathLike[str] | None, str]:
    """Converts the specified trace file to JSON.

    Args:
      trace_path: The path to the trace file to convert.
      trace2json_path: The path to the trace2json executable. When unset, find
          at a runtime_deps/trace2json location in a parent directory.
      patterns: Regexps to match against event names. Only events that match one or more of these
                patterns will be included in the converted trace file.
                Pass None to include all events.
                Pass the empty set to discard all events.
      categories: The names of categories to include. An event will be included if it either
                  matches one of the elements of `patterns`, OR it is in one of the categories
                  listed here.
                  Pass None or the empty set to defer entirely to `patterns`.
      split: Whether to split `systemTraceEvents` out into a separate, jsonlines-formatted, file.

    Raises:
      subprocess.CalledProcessError: The trace2json process returned an error.

    Returns:
      The path to the converted trace file.
      The path to the split-out jsonlines file, if requested, or the empty string.
      Usage stats of the conversion process.
    """
    _LOGGER.info(f"Converting {trace_path} to json")

    output_path = pathlib.Path(trace_path).with_suffix(".json")
    jsonlines_path = (
        output_path.with_suffix(_JSONLINES_SUFFIX) if split else None
    )

    with (
        as_file(files(data).joinpath("trace2json")) as trace2json_res,
        as_file(files(data).joinpath("process_monitor")) as process_monitor_res,
    ):
        if trace2json_path is None:
            trace2json_res.chmod(trace2json_res.stat().st_mode | stat.S_IEXEC)
            trace2json_path = trace2json_res

        process_monitor_res.chmod(
            process_monitor_res.stat().st_mode | stat.S_IEXEC
        )

        args: list[str | os.PathLike[str]] = [
            process_monitor_res,
            trace2json_path,
            f"--input-file={trace_path}",
            f"--output-file={output_path}",
        ]
        if jsonlines_path:
            args.append(f"--system-event-output-file={jsonlines_path}")

        # patterns can be None, the empty set, or a set containing some patterns.
        # If the caller specified None (or one of their patterns is ".*"), they want all events.
        # If the caller explicitly asked for the empty set, trace2json must drop all events.
        #   We can achieve this by using an "impossible pattern", a pattern which can't match
        #   anything, like "[^\d\D]" which only matches if characters both are and are not digits.
        # Otherwise, use their provided patterns as filters.
        if patterns is None or r".*" in patterns:
            patterns = set()
        elif len(patterns) == 0:
            patterns = {r"[^\d\D]"}
        args.extend([f"--pattern={pattern}" for pattern in patterns])

        if categories:
            args.extend([f"--category={category}" for category in categories])

        _LOGGER.info(f"Running {args}")
        try:
            conversion_output = subprocess.check_output(
                args, text=True, stderr=subprocess.STDOUT
            )

        except subprocess.CalledProcessError as cpe:
            _LOGGER.error("trace2json failed: %s", cpe.stdout)
            raise
        _LOGGER.debug("Output of running %s: %s", args, conversion_output)

    return (output_path, jsonlines_path, conversion_output)


def create_model_from_trace_file_path(
    trace_path: str | os.PathLike[Any],
    trace2json_path: str | os.PathLike[Any] | None = None,
    patterns: Set[str] | None = None,
    categories: Set[str] | None = None,
    without_json_conversion: bool = False,
) -> trace_model.Model:
    """Converts the specified trace file to JSON.

    Args:
      trace_path: The path to the trace file to convert.
      trace2json_path: The path to the trace2json executable. When unset, find
          at a runtime_deps/trace2json location in a parent directory.
      patterns: Regexps to match against event names. Only events that match one or more of these
                patterns will be included in the converted trace file.
                Pass None to include all events.
                Pass the empty set to discard all events.
      categories: The names of categories to include. An event will be included if it either
                  matches one of the elements of `patterns`, OR it is in one of the categories
                  listed here.
                  Pass None or the empty set to defer entirely to `patterns`.
      without_json_conversion: Whether to skip the json conversion and use the trace processor shell to query the trace file for events instead.

    Raises:
      subprocess.CalledProcessError: The trace2json process returned an error.

    Returns:
        A Model object.
    """
    if without_json_conversion:
        # Import lazily to avoid circular dependency between trace_importing and trace_importing_fxt.
        from trace_processing import trace_importing_fxt

        return trace_importing_fxt.create_model_from_fxt_path_directly(
            str(trace_path), patterns, categories
        )
    return create_model_from_file_path(
        convert_trace_file_to_json(
            trace_path, trace2json_path, patterns, categories
        )
    )


def create_model_using_tp_shell(
    session: PerfettoTraceProcessor,
    patterns: Set[str] | None = None,
    categories: Set[str] | None = None,
) -> trace_model.Model:
    """Creates a trace model directly from an active PerfettoTraceProcessor session.

    Args:
        session: Active PerfettoTraceProcessor instance.
        patterns: Optional regex patterns to filter events.
        categories: Optional categories to filter events.

    Returns:
        A Model object.
    """
    from trace_processing import trace_importing_fxt

    return trace_importing_fxt.create_model_from_tp_session(
        session, patterns, categories
    )


def create_model_from_file_path(
    path: str | os.PathLike[Any],
) -> trace_model.Model:
    """Create a Model from a file path.

    Args:
        path: The path to the file.

    Returns:
        A Model object.
    """

    with open(path, "r") as file:
        return create_model_from_file(file)


def create_model_from_file(file: TextIO) -> trace_model.Model:
    """Create a Model from a file.

    Args:
        file: The file to read.

    Returns:
        A Model object.
    """

    return consume_json_to_create_model(json.load(file))


def create_model_from_string(json_string: str) -> trace_model.Model:
    """Create a Model from a raw JSON string of trace data.

    Args:
        json_string: The JSON string to parse.

    Returns:
        A Model object.
    """

    return consume_json_to_create_model(json.loads(json_string))


def _validate_field_type(
    d: Dict[str, Any], field: str, ty: type | types.UnionType
) -> None:
    """
    Check that a given field exists in the dictionary and has the expected type
    """
    if not (field in d and isinstance(d[field], ty)):
        raise TypeError(f"Expected {d} to have field '{field}' of type '{ty}'")


def create_model_from_file_paths(
    trace_events_path: str | os.PathLike[Any],
    systrace_events_path: str | os.PathLike[Any],
) -> trace_model.Model:
    """Create a Model from paths to split JSON trace files.

    Though it's clunkier to support multiple files to represent a single trace, this approach uses
    significantly less RAM -- especially for traces that are scheduler-record-heavy. Since the
    scheduler records (and process and thread records) are in a separate jsonlines-formatted file,
    the records can be loaded, ingested and discarded one by one. When the whole trace lives in a
    single file, the ENTIRE JSON object must be loaded into memory for at least part of the
    trace-processing time.

    Args:
        trace_events_path: Path to the Chromium-formatted JSON trace file.
        systrace_events_path: Path to jsonlines-formatted system events file.

    Returns:
        A Model object.
    """

    pid_to_name: dict[int, str] = {}
    tid_to_name: dict[int, str] = {}
    tid_to_pid: dict[int, int] = {}

    with open(trace_events_path, "r") as file:
        result_events = _consume_json_for_trace_events(
            json.load(file), pid_to_name, tid_to_name
        )

    with open(systrace_events_path, "r") as jsonlines_file:
        scheduling_records: dict[int, list[trace_model.SchedulingRecord]] = {}
        for line in jsonlines_file:
            system_trace_event = json.loads(line)
            _validate_field_type(system_trace_event, "ph", str)
            _ingest_system_record(
                system_trace_event,
                pid_to_name,
                tid_to_name,
                tid_to_pid,
                scheduling_records,
            )

    return construct_model(
        pid_to_name, tid_to_name, tid_to_pid, result_events, scheduling_records
    )


def _consume_json_for_trace_events(
    root_object: dict[str, Any],
    pid_to_name: dict[int, str],
    tid_to_name: dict[int, str],
) -> list[trace_model.Event]:
    """Destructively creates a list of trace events from a JSON dictionary.

    Args:
        root_object: A JSON dictionary representing the trace data. This function takes ownership of
                     the provided JSON object.
        pid_to_name: Map of process IDs to process names. Updated in-place.
        tid_to_name: Map of thread IDs to thread names. Updated in-place.

    Returns:
        A Model object.
    """

    # A helper lambda to assert that expected fields in a JSON trace event are
    # present and are of the correct type.  If any of these fields are missing
    # or is of a different type than what is asserted here, then the JSON trace
    # event is considered to be malformed.
    def check_trace_event(json_trace_event: Dict[str, Any]) -> None:
        _validate_field_type(json_trace_event, "ph", str)
        if json_trace_event["ph"] != "M":
            _validate_field_type(json_trace_event, "cat", str)
        _validate_field_type(json_trace_event, "name", str)
        if json_trace_event["ph"] != "M":
            _validate_field_type(json_trace_event, "ts", float | int)
        _validate_field_type(json_trace_event, "pid", int)
        _validate_field_type(json_trace_event, "tid", float | int)
        if "args" in json_trace_event:
            _validate_field_type(json_trace_event, "args", dict)

    # A helper lambda to add duration events to the appropriate duration stack
    # and do the appropriate duration/flow graph setup.  It is used for both
    # begin/end pairs and complete events.
    def add_to_duration_stack(
        duration_event: trace_model.DurationEvent,
        duration_stack: List[trace_model.DurationEvent],
    ) -> None:
        duration_stack.append(duration_event)
        if len(duration_stack) > 1:
            top = duration_stack[-1]
            top_parent = duration_stack[-2]
            top.parent = top_parent
            top_parent.child_durations.append(duration_event)

    # Obtain the overall list of trace events.
    _validate_field_type(root_object, "traceEvents", list)
    trace_events: List[Dict[str, Any]] = root_object["traceEvents"].copy()

    # Add synthetic end events for each complete event in the trace data to
    # assist with maintaining each thread's duration stack.  This isn't strictly
    # necessary, however it makes the duration stack bookkeeping simpler.
    for trace_event in root_object["traceEvents"]:
        if trace_event["ph"] == "X":
            synthetic_end_event: Dict[str, Any] = trace_event.copy()
            synthetic_end_event["ph"] = "fuchsia_synthetic_end"
            synthetic_end_event["ts"] = trace_event["ts"] + trace_event["dur"]
            trace_events.append(synthetic_end_event)
    del root_object["traceEvents"]

    # Sort the events by their timestamp.  We need to iterate through the events
    # in sorted order to compute things such as duration stacks and flow
    # sequences. Events without timestamps (e.g. Chrome's metadata events) are
    # sorted to the beginning.
    #
    # We need to use a stable sort here, which fortunately `list.sort` is. If we
    # use a non-stable sort, zero-length duration events of type ph='X' are not
    # handled properly, because the 'fuchsia_synthetic_end' events can get
    # sorted before their corresponding beginning events.
    trace_events.sort(key=lambda x: x.get("ts", 0))

    del root_object

    # Maintains the current duration stack for each track.
    duration_stacks: Dict[
        Tuple[int, int], List[trace_model.DurationEvent]
    ] = defaultdict(list)
    # Maintains in progress async events.
    live_async_events: Dict[_AsyncKey, trace_model.AsyncEvent] = {}
    # Maintains in progress flow sequences.
    live_flows: Dict[_FlowKey, trace_model.FlowEvent] = {}
    # Flows with "next slide" binding that are waiting to be bound.
    unbound_flow_events: Dict[
        Tuple[int, int], List[trace_model.FlowEvent]
    ] = defaultdict(list)
    # Final list of events to be written into the Model.
    result_events: List[trace_model.Event] = []

    dropped_flow_event_counter: int = 0
    dropped_async_event_counter: int = 0
    # TODO(https://fxbug.dev/42117378): Support nested async events.  In the meantime, just
    # drop them.
    dropped_nested_async_event_counter: int = 0

    # Create result events from trace events.
    for trace_event in trace_events:
        # Guarantees trace event will have certain fields present, so they are
        # not checked below.
        check_trace_event(trace_event)

        phase: str = trace_event["ph"]
        pid: int = trace_event["pid"]
        tid: int = int(trace_event["tid"])
        track_key: Tuple[int, int] = (pid, tid)
        duration_stack = duration_stacks[track_key]

        if phase in ("X", "B"):
            duration_event = trace_model.DurationEvent.consume_dict(trace_event)
            if track_key in unbound_flow_events:
                for unbound_flow_event in unbound_flow_events[track_key]:
                    unbound_flow_event.enclosing_duration = duration_event
                unbound_flow_events[track_key].clear()
            add_to_duration_stack(duration_event, duration_stack)
            if phase == "X":
                result_events.append(duration_event)
        elif phase == "E":
            if duration_stack:
                popped_begin: trace_model.DurationEvent = duration_stack.pop()
                # It's we drop the "Begin" or "End" part of a begin-end duration pair, or we get
                # mismatched Begin-End pairs in general. We should attempt some form of error
                # recovery. It's likely this is a local error due to either dropped events. The rest
                # of the trace is likely still good.
                if popped_begin.duration != None:
                    # We know that since we artificially insert the matching pairs for complete
                    # duration events (ph == X), those must be correct and we can attempt to recover
                    # using them. If we're attempting to match with a duration complete event (i.e.
                    # it has a duration set already), drop this end event instead
                    duration_stack.append(popped_begin)
                    continue
                popped_begin.duration = (
                    trace_time.TimePoint.from_epoch_delta(
                        trace_time.TimeDelta.from_microseconds(
                            trace_event["ts"]
                        )
                    )
                    - popped_begin.start
                )
                if "args" in trace_event:
                    popped_begin.args = {
                        **popped_begin.args,
                        **trace_event["args"],
                    }
                result_events.append(popped_begin)
        elif phase == "fuchsia_synthetic_end":
            assert duration_stack
            popped_complete: trace_model.DurationEvent = duration_stack.pop()
            # We know that since we artificially insert the matching pairs for complete duration
            # there must be a matching event. If we popped a non duration complete begin event (i.e.
            # it has no duration set yet), we must have dropped a matching end somewhere.
            #
            # We'll attempt to recover by popping events until we find a duration complete begin
            # event.
            while popped_complete.duration == None:
                popped_complete = duration_stack.pop()
        elif phase == "b":
            async_key: _AsyncKey = _AsyncKey.from_trace_event(trace_event)
            async_event = trace_model.AsyncEvent.consume_dict(
                async_key.id, trace_event
            )
            live_async_events[async_key] = async_event
        elif phase == "e":
            async_key = _AsyncKey.from_trace_event(trace_event)
            begin_async_event: Optional[
                trace_model.AsyncEvent
            ] = live_async_events.pop(async_key, None)
            if begin_async_event is not None:
                begin_async_event.duration = (
                    trace_time.TimePoint.from_epoch_delta(
                        trace_time.TimeDelta.from_microseconds(
                            trace_event["ts"]
                        )
                    )
                    - begin_async_event.start
                )
                if "args" in trace_event:
                    begin_async_event.args = {
                        **begin_async_event.args,
                        **trace_event["args"],
                    }
            else:
                dropped_async_event_counter += 1
                continue
            result_events.append(begin_async_event)
        elif phase == "i" or phase == "I":
            instant_event = trace_model.InstantEvent.consume_dict(trace_event)
            result_events.append(instant_event)
        elif phase == "s" or phase == "t" or phase == "f":
            binding_point: Optional[str] = None
            if "bp" in trace_event:
                if trace_event["bp"] == "e":
                    binding_point = "enclosing"
                else:
                    raise TypeError(
                        f"Found unexpected value in bp field of {trace_event}"
                    )
            elif phase == "s" or phase == "t":
                binding_point = "enclosing"
            elif phase == "f":
                binding_point = "next"

            flow_key: _FlowKey = _FlowKey.from_trace_event(trace_event)
            previous_flow: Optional[trace_model.FlowEvent] = None
            if phase == "s":
                if flow_key in live_flows:
                    dropped_flow_event_counter += 1
                    continue
            elif phase == "t" or phase == "f":
                previous_flow = live_flows.get(flow_key, None)
                if previous_flow is None:
                    dropped_flow_event_counter += 1
                    continue

            if not duration_stack:
                dropped_flow_event_counter += 1
                continue

            enclosing_duration: Optional[trace_model.DurationEvent] = (
                duration_stack[-1] if binding_point == "enclosing" else None
            )
            flow_event = trace_model.FlowEvent.consume_dict(
                flow_key.id, enclosing_duration, trace_event
            )
            if enclosing_duration:
                enclosing_duration.child_flows.append(flow_event)
            else:
                unbound_flow_events[track_key].append(flow_event)

            if previous_flow is not None:
                previous_flow.next_flow = flow_event
            flow_event.previous_flow = previous_flow

            if phase == "s" or phase == "t":
                live_flows[flow_key] = flow_event
            else:
                live_flows.pop(flow_key)
            result_events.append(flow_event)
        elif phase == "C":
            counter_event = trace_model.CounterEvent.consume_dict(trace_event)
            result_events.append(counter_event)
        elif phase == "n":
            # TODO(https://fxbug.dev/42117378): Support nested async events.  In the
            # meantime, just drop them.
            dropped_nested_async_event_counter += 1
        elif phase == "M":
            # Chrome metadata events. These define process and thread names,
            # similar to the Fuchsia systemTraceEvents.
            if trace_event["name"] == "process_name":
                # If trace_event contains args, those are verified to be of type
                # dict in check_trace_event.
                if (
                    "args" not in trace_event
                    or "name" not in trace_event["args"]
                ):
                    raise TypeError(
                        f"{trace_event} is a process_name metadata event but "
                        f"doesn't have a name argument"
                    )
                pid_to_name[pid] = trace_event["args"]["name"]
            if trace_event["name"] == "thread_name":
                # If trace_event contains args, those are verified to be of type
                # dict in check_trace_event.
                if (
                    "args" not in trace_event
                    or "name" not in trace_event["args"]
                ):
                    raise TypeError(
                        f"{trace_event} is a thread_name metadata event but "
                        f"doesn't have a name argument"
                    )
                tid_to_name[tid] = trace_event["args"]["name"]
        elif phase in ("R", "(", ")", "O", "N", "D", "S", "T", "p", "F"):
            # Ignore some phases that are in Chrome traces that we don't yet
            # have use cases for.
            #
            # These are:
            # * 'R' - Mark events, similar to instants created by the Navigation
            #         Timing API
            # * '(', ')' - Context events
            # * 'O', 'N', 'D' - Object events
            # * 'S', 'T', 'p', 'F' - Legacy async events
            pass
        else:
            raise TypeError(
                f"Encountered unknown phase {phase} from {trace_event}"
            )

    # Maintaining a copy of all trace events and synthetic events is costly, so
    # release the associated memory proactively.
    del trace_events

    # Sort events by their start timestamp.
    #
    # We need a stable sort here, which fortunately `list.sort` is.  This is
    # required to preserve the ordering of events when they share the same start
    # timestamp. Such events are more likely to occur on systems with a low
    # timer resolution.
    result_events.sort(key=lambda x: x.start)

    # Print warnings about anomalous conditions in the trace.
    live_duration_events_count = sum(len(ds) for ds in duration_stacks.values())
    if live_duration_events_count > 0:
        _LOGGER.warning(
            f"Warning, finished processing trace events with "
            f"{live_duration_events_count} in progress duration events"
        )
    if live_async_events:
        _LOGGER.warning(
            f"Warning, finished processing trace events with "
            f"{len(live_async_events)} in progress async events"
        )
    if live_flows:
        _LOGGER.warning(
            f"Warning, finished processing trace events with {len(live_flows)} "
            f"in progress flow events"
        )
    if dropped_async_event_counter > 0:
        _LOGGER.warning(
            f"Warning, dropped {dropped_async_event_counter} async events"
        )
    if dropped_flow_event_counter > 0:
        _LOGGER.warning(
            f"Warning, dropped {dropped_flow_event_counter} flow events"
        )
    if dropped_nested_async_event_counter > 0:
        _LOGGER.warning(
            f"Warning, dropped {dropped_nested_async_event_counter} nested "
            f"async events"
        )

    return result_events


def consume_json_to_create_model(
    root_object: dict[str, Any]
) -> trace_model.Model:
    """Destructively creates a Model from a JSON dictionary.

    Args:
        root_object: A JSON dictionary representing the trace data. This function takes ownership of
                     the provided JSON object.


    Returns:
        A Model object.
    """
    # Pull system trace events out _before_ handing ownership of root_object off
    # to _consume_json_for_trace_events()
    system_trace_events_list: dict[str, Any] = {}
    if "systemTraceEvents" in root_object:
        _validate_field_type(root_object, "systemTraceEvents", dict)
        system_trace_events_list = root_object["systemTraceEvents"]
        del root_object["systemTraceEvents"]
    scheduling_records: dict[int, list[trace_model.SchedulingRecord]] = {}

    pid_to_name: dict[int, str] = {}
    tid_to_name: dict[int, str] = {}
    result_events = _consume_json_for_trace_events(
        root_object, pid_to_name, tid_to_name
    )
    del root_object

    # Map pid -> tid because some of these subclasses (such as ContextSwitch)
    # need to use the mapping but don't have the pid field.
    tid_to_pid: Dict[int, int] = {}

    # Process system trace events.
    if system_trace_events_list:
        _validate_field_type(system_trace_events_list, "type", str)
        if not system_trace_events_list["type"] == "fuchsia":
            raise TypeError(
                f"Expected {system_trace_events_list} to have field 'type' "
                f"equal to value 'fuchsia'"
            )
        _validate_field_type(system_trace_events_list, "events", list)
        for system_trace_event in system_trace_events_list["events"]:
            _ingest_system_record(
                system_trace_event,
                pid_to_name,
                tid_to_name,
                tid_to_pid,
                scheduling_records,
            )

    return construct_model(
        pid_to_name, tid_to_name, tid_to_pid, result_events, scheduling_records
    )


def _ingest_system_record(
    system_trace_event: dict[str, Any],
    pid_to_name: dict[int, str],
    tid_to_name: dict[int, str],
    tid_to_pid: dict[int, int],
    scheduling_records: dict[int, list[trace_model.SchedulingRecord]],
) -> None:
    _validate_field_type(system_trace_event, "ph", str)

    system_event_type: str = system_trace_event["ph"]
    if system_event_type == "p":
        pid, name = _ingest_process_record(system_trace_event)
        pid_to_name[pid] = name
    elif system_event_type == "t":
        tid, pid, name = _ingest_thread_record(system_trace_event)
        tid_to_name[tid] = name
        tid_to_pid[tid] = pid
    elif system_event_type == "k":
        cpu, switch_record = _ingest_context_switch(system_trace_event)
        scheduling_records.setdefault(cpu, [])
        scheduling_records[cpu].append(switch_record)
    elif system_event_type == "w":
        cpu, waking_record = _ingest_waking_record(system_trace_event)
        scheduling_records.setdefault(cpu, [])
        scheduling_records[cpu].append(waking_record)
    else:
        _LOGGER.warning(
            f"Unknown phase {system_event_type} from {system_trace_event}"
        )


def construct_model(
    pid_to_name: dict[int, str],
    tid_to_name: dict[int, str],
    tid_to_pid: dict[int, int],
    result_events: list[trace_model.Event],
    scheduling_records: dict[int, list[trace_model.SchedulingRecord]],
) -> trace_model.Model:
    # Construct the map of Processes, including ones without trace events.

    # Maps from PIDs to Process objects.
    processes: Dict[int, trace_model.Process] = {}
    # Maps from Process objects to dicts that map from TIDs to Thread objects.
    process_threads_map: Dict[
        trace_model.Process, Dict[int, trace_model.Thread]
    ] = {}

    def get_process(pid: int) -> trace_model.Process:
        if pid in processes:
            return processes[pid]
        process = trace_model.Process(pid=pid)
        processes[pid] = process
        return process

    def get_thread(
        process: trace_model.Process, tid: int
    ) -> trace_model.Thread:
        threads_map = process_threads_map.setdefault(process, {})
        if tid in threads_map:
            return threads_map[tid]
        thread = trace_model.Thread(
            tid=tid, name=tid_to_name.get(tid, "tid: %d" % tid)
        )
        threads_map[tid] = thread
        process.threads.append(thread)
        return thread

    for event in result_events:
        thread = get_thread(get_process(event.pid), event.tid)
        thread.events.append(event)

    for tid, pid in tid_to_pid.items():
        # If we don't already have the thread from the result_events loop, add it now.
        get_thread(get_process(pid), tid)

    # Construct the final Model.
    processes_sorted_by_pid = []
    for pid, process in sorted(processes.items()):
        process.threads.sort(key=lambda thread: thread.tid)
        if process.pid in pid_to_name:
            process.name = pid_to_name[process.pid]
        processes_sorted_by_pid.append(process)

    return trace_model.Model(processes_sorted_by_pid, scheduling_records)


def _ingest_process_record(system_trace_event: dict[str, Any]) -> PidMapping:
    _validate_field_type(system_trace_event, "pid", int)
    _validate_field_type(system_trace_event, "name", str)

    pid = system_trace_event["pid"]
    name = system_trace_event["name"]
    return PidMapping(pid, name)


def _ingest_thread_record(system_trace_event: dict[str, Any]) -> TidMapping:
    _validate_field_type(system_trace_event, "pid", int)
    _validate_field_type(system_trace_event, "name", str)
    _validate_field_type(system_trace_event, "tid", float | int)

    return TidMapping(
        int(system_trace_event["tid"]),
        system_trace_event["pid"],
        system_trace_event["name"],
    )


def _ingest_context_switch(
    system_trace_event: dict[str, Any]
) -> ContextSwitchMapping:
    """Ingest a context switch record.

    Context Switch Records

    Contains data about when a thread was scheduled on a cpu.
    The incoming or outgoing thread may be the idle thread, which is indicated by a
    priority of -0x80000000.
    """
    _validate_field_type(system_trace_event, "ts", float | int)
    _validate_field_type(system_trace_event, "cpu", int)
    _validate_field_type(system_trace_event, "out", dict)
    _validate_field_type(system_trace_event["out"], "tid", int)
    _validate_field_type(system_trace_event["out"], "state", int)
    _validate_field_type(system_trace_event, "in", dict)
    _validate_field_type(system_trace_event["in"], "tid", int)

    incoming_prio = None
    outgoing_prio = None

    if "prio" in system_trace_event["in"] and isinstance(
        system_trace_event["in"]["prio"], int
    ):
        incoming_prio = system_trace_event["in"]["prio"]

    if "prio" in system_trace_event["out"] and isinstance(
        system_trace_event["out"]["prio"], int
    ):
        outgoing_prio = system_trace_event["out"]["prio"]

    timestamp = trace_time.TimePoint.from_epoch_delta(
        trace_time.TimeDelta.from_microseconds(system_trace_event["ts"])
    )

    cpu = system_trace_event["cpu"]
    incoming_tid = system_trace_event["in"]["tid"]
    outgoing_tid = system_trace_event["out"]["tid"]
    outgoing_state = system_trace_event["out"]["state"]
    args = system_trace_event.get("args", {})

    return ContextSwitchMapping(
        cpu,
        trace_model.ContextSwitch(
            start=timestamp,
            incoming_tid=incoming_tid,
            outgoing_tid=outgoing_tid,
            incoming_prio=incoming_prio,
            outgoing_prio=outgoing_prio,
            outgoing_state=trace_model.ThreadState(outgoing_state),
            args=args,
        ),
    )


def _ingest_waking_record(system_trace_event: dict[str, Any]) -> WakingMapping:
    """Ingest a waking record for a thread.
    Waking Record

    Indicates that thread has woken up and is waiting to run on a given cpu. Frequent
    and lengthy waking records can indicate high cpu contention or starving threads.
    """
    _validate_field_type(system_trace_event, "ts", float | int)
    _validate_field_type(system_trace_event, "cpu", int)
    _validate_field_type(system_trace_event, "tid", int)

    # Args and prio are optional, if they exist, make sure they are correct
    prio = None
    if "prio" in system_trace_event:
        _validate_field_type(system_trace_event, "prio", int)
        prio = system_trace_event["prio"]

    if "args" in system_trace_event:
        _validate_field_type(system_trace_event, "args", dict)

    timestamp = trace_time.TimePoint.from_epoch_delta(
        trace_time.TimeDelta.from_microseconds(system_trace_event["ts"])
    )
    cpu = system_trace_event["cpu"]
    tid = system_trace_event["tid"]
    args = system_trace_event.get("args", {})
    return WakingMapping(
        cpu, trace_model.Waking(start=timestamp, tid=tid, prio=prio, args=args)
    )
