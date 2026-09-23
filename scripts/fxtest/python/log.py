# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import collections
from collections.abc import Iterator
from dataclasses import dataclass
import errno
import fcntl
import gzip
import json
import math
import os
import statistics
import sys
import typing
import zlib

import environment
import event
import selection

_SUMMARY_TOP_N_MAX_COUNT = 5
_SUMMARY_TOP_N_CUTOFF_SECONDS = 1.0


def format_fallback_path(path: str, pid: int) -> str:
    """Inserts .<pid> before compression extensions so tools still recognize the file."""
    for ext in (".log.json.gz", ".json.gz", ".gz"):
        if path.endswith(ext):
            base = path[: -len(ext)]
            return f"{base}.{pid}{ext}"
    return f"{path}.{pid}"


_LOCK_CONTENTION_ERRNOS = (errno.EACCES, errno.EAGAIN, errno.EWOULDBLOCK)


def _open_locked_file(path: str) -> typing.TextIO:
    """Attempts to open, exclusively lock, and truncate a single gzip log file.

    Raises:
        OSError: If opening, locking, or truncating fails. The file descriptor
            is guaranteed to be closed on failure.
    """
    parent_dir = os.path.dirname(path)
    if parent_dir:
        os.makedirs(parent_dir, exist_ok=True)

    # Open without O_TRUNC to avoid clobbering active writers before acquiring the lock.
    fd = os.open(path, os.O_RDWR | os.O_CREAT, 0o644)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)

        # Acquired exclusive lock. Truncate to overwrite cleanly.
        os.ftruncate(fd, 0)
        os.lseek(fd, 0, os.SEEK_SET)

        raw_file = os.fdopen(fd, "wb")
        return typing.cast(typing.TextIO, gzip.open(raw_file, "wt"))
    except OSError:
        os.close(fd)
        raise


def open_exclusive_log_file(desired_path: str) -> tuple[typing.TextIO, str]:
    """Opens a log file for writing with an exclusive advisory lock.

    If nobody is writing to desired_path, it is exclusively locked and overwritten.
    If another process holds a write lock, appends the PID to the filename
    and opens that instead to avoid corrupting concurrent runs.

    Returns:
        A tuple of (writable_gzip_text_stream, actual_file_path).
    """
    try:
        return _open_locked_file(desired_path), desired_path
    except OSError as e:
        # Only fall back if the failure was due to lock contention from another process.
        # For other errors (e.g. disk full, bad directory permissions), re-raise immediately.
        if e.errno not in _LOCK_CONTENTION_ERRNOS:
            raise

        # Try once more with the PID-suffixed fallback path.
        # This call is deliberately not caught: if the fallback also fails,
        # we have exhausted our fallback options, so let the error propagate.
        fallback_path = format_fallback_path(desired_path, os.getpid())
        return _open_locked_file(fallback_path), fallback_path


async def writer(
    recorder: event.EventRecorder,
    out_stream: typing.TextIO,
) -> None:
    """Asynchronously serialize events to the given stream.

    Args:
        recorder (event.EventRecorder): The source of events to
            drain. Continues until all events are written.
        out_stream (typing.TextIO): Output text stream.
    """
    value: event.Event

    async for value in recorder.iter():
        try:
            json.dump(value.to_dict(), out_stream)  # type:ignore
            out_stream.write("\n")
            # Eagerly flush after each line. This task may terminate at any time,
            # including from an interrupt, so this ensures we at least see
            # the most recently written lines.
            out_stream.flush()
        except TypeError as e:
            print(f"LOG ERROR: {e} {value}")


@dataclass
class LogIterElement:
    # If set, this element provides the path of the log that was opened.
    log_path: str | None = None

    # If set, this elements provides one event from the file.
    log_event: event.Event | None = None

    # If set, this element provides a warning message. Parsing should continue.
    warning: str | None = None

    # If set, this element provides a fatal error message. Parsing should stop.
    error: str | None = None


class LogSource:
    __static_key = object()

    def __init__(
        self,
        __static_key: typing.Any,
        exec_env: environment.ExecutionEnvironment | None = None,
        stream: typing.TextIO | None = None,
    ):
        assert (
            __static_key == self.__static_key
        ), "LogSource must be created by from_env() or from_stream()."
        assert (
            exec_env is None or stream is None
        ), "LogSource must be either an environment or stream."
        self._exec_env = exec_env
        self._stream = stream

    @classmethod
    def from_env(
        cls, exec_env: environment.ExecutionEnvironment
    ) -> "LogSource":
        return LogSource(cls.__static_key, exec_env=exec_env)

    @classmethod
    def from_stream(cls, stream: typing.TextIO | None = None) -> "LogSource":
        return LogSource(cls.__static_key, stream=stream)

    def read_log(
        self,
        event_filter: (
            typing.Callable[[dict[str, typing.Any]], bool] | None
        ) = None,
        line_filter: (typing.Callable[[str], bool] | None) = None,
    ) -> Iterator[LogIterElement]:
        stream = self._stream
        close_stream = False

        try:
            if stream is None and self._exec_env is not None:
                log_path = self._exec_env.get_most_recent_log()
                yield LogIterElement(log_path=log_path)
                stream = gzip.open(log_path, "rt")
                close_stream = True

            assert stream is not None

            for line in stream:
                if not line:
                    continue

                if line_filter is not None and not line_filter(line):
                    continue

                json_contents = json.loads(line)
                if event_filter is not None and not event_filter(json_contents):
                    continue
                log_event: event.Event = event.Event.from_dict(json_contents)  # type: ignore[attr-defined]
                yield LogIterElement(log_event=log_event)
        except environment.EnvironmentError as e:
            yield LogIterElement(error=f"Failed to read log: {e}")
            return
        except gzip.BadGzipFile as e:
            yield LogIterElement(
                error=f"File does not appear to be a gzip file. ({e})"
            )
            return
        except json.JSONDecodeError as e:
            yield LogIterElement(
                warning=f"Found invalid JSON data, skipping the rest and proceeding. ({e})"
            )
        except (EOFError, zlib.error) as e:
            yield LogIterElement(
                warning=f"File may be corrupt, skipping the rest and proceeding. ({e})",
            )
        finally:
            if close_stream and stream is not None:
                stream.close()


def pretty_print(
    log_source: LogSource,
) -> bool:
    suite_names: dict[int, str] = dict()
    command_to_suite: dict[int, int] = dict()

    formatted_suite_events: collections.defaultdict[
        int, list[str]
    ] = collections.defaultdict(list)

    time_base: float | None = None

    def format_time(e: event.Event) -> str:
        nonlocal time_base
        if time_base is None:
            time_base = e.timestamp
        ts = max(0.0, e.timestamp - time_base)
        seconds = math.floor(ts)
        millis = int(ts * 1e3 % 1e3)
        return f"{seconds:04}.{millis:03}"

    for element in log_source.read_log():
        if (e := element.log_event) is not None:
            if e.id == 0:
                time_base = e.timestamp
            if not e.payload:
                continue
            if ex := e.payload.test_suite_started:
                assert e.id
                suite_names[e.id] = ex.name
                formatted_suite_events[e.id].append(
                    f"[{format_time(e)}] Starting suite {ex.name}"
                )
            if (
                (pid := e.parent)
                and pid in suite_names
                and (command := e.payload.program_execution)
            ):
                assert e.id
                command_to_suite[e.id] = pid
                args = " ".join([command.command] + command.flags)
                env = command.environment
                formatted_suite_events[pid].append(
                    f"[{format_time(e)}] Running command\n  Args: {args}\n   Env: {env}"
                )
            if e.id in command_to_suite:
                if output := e.payload.program_output:
                    formatted_suite_events[command_to_suite[e.id]].append(
                        f"[{format_time(e)}] {output.data}"
                    )
                if termination := e.payload.program_termination:
                    formatted_suite_events[command_to_suite[e.id]].append(
                        f"[{format_time(e)}] Command terminated: {termination.return_code}"
                    )
            if e.id in suite_names and (outcome := e.payload.test_suite_ended):
                formatted_suite_events[e.id].append(
                    f"[{format_time(e)}] Suite ended with status {outcome.status.value}"
                )
        elif (warning := element.warning) is not None:
            print(warning, file=sys.stderr)
        elif (error := element.error) is not None:
            print(error, file=sys.stderr)
            return False
    print(f"{len(suite_names)} tests were run")
    for id in sorted(suite_names.keys()):
        name = suite_names[id]
        print(f"\n[START {name}]")
        for line in formatted_suite_events[id]:
            print(line.strip())
        print(f"[END {name}]\n")
    return True


_CATEGORY_CODE_MAP = {
    event.EventStatCategory.TESTING: "T",
    event.EventStatCategory.BUILDING: "B",
    event.EventStatCategory.PARSING: "P",
    event.EventStatCategory.SEARCHING: "S",
    event.EventStatCategory.OTHERS: "O",
}


def _clean_label(label: str) -> str:
    label = label.removeprefix("Running TestSuite ")
    label = label.removeprefix("fuchsia-pkg://fuchsia.com/")
    return label[:70]


def _to_micros(seconds: float) -> int:
    return int(round(seconds * 1_000_000))


@dataclass
class AbortedSpan:
    label: str
    elapsed_micros: int
    category: str


@dataclass
class TopNEvent:
    label: str
    duration: float
    category: event.EventStatCategory | None


@dataclass
class CategoryStats:
    sum: float
    count: int
    mean: float
    std: float


@dataclass
class ExecutionStats:
    top_n: list[TopNEvent]
    summary: dict[event.EventStatCategory, CategoryStats]
    was_aborted: bool = False
    total_elapsed_micros: int = 0
    aborted_span: AbortedSpan | None = None
    selection: str = ""

    def to_analytics_dict(self) -> dict[str, typing.Any]:
        """Convert execution stats to a compact dictionary for analytics reporting.

        Short keys and codes are used to minimize payload size:

        Top-level keys:
            t: total_elapsed_micros - Total elapsed time in microseconds (int).
            a: was_aborted - 1 if execution was aborted, 0 if completed (int).
            ab: aborted_span - Dict with details of span active during abort, or None.
            top: top_n - List of top slowest operations (list[dict]).
            sum: summary - Aggregated metrics grouped by category code (dict[str, dict]).
            sel: selection - Canonical test selection string (str).

        Aborted span ('ab') keys:
            l: label - Cleaned label of the aborted operation (str).
            d: duration - Elapsed time before abort in microseconds (int).
            c: category - Category short code (e.g. "T", "B", "O") (str).

        Top operation ('top') item keys:
            l: label - Cleaned label of the operation (str).
            d: duration - Operation duration in microseconds (int).
            c: category - Category short code ("T", "B", "P", "S", "O") (str).

        Summary ('sum') category codes:
            T: Testing (EventStatCategory.TESTING)
            B: Building (EventStatCategory.BUILDING)
            P: Parsing (EventStatCategory.PARSING)
            S: Searching (EventStatCategory.SEARCHING)
            O: Others (EventStatCategory.OTHERS)

        Summary metric keys (under each category in 'sum'):
            s: sum - Total cumulative duration in microseconds (int).
            c: count - Number of events in this category (int).
            m: mean - Average event duration in microseconds (int).
            sd: std - Standard deviation of event durations in microseconds (int).

        Returns:
            Dictionary with short keys suitable for analytics serialization.
        """
        aborted_dict = None
        if self.aborted_span is not None:
            aborted_dict = {
                "l": _clean_label(self.aborted_span.label),  # label
                "d": self.aborted_span.elapsed_micros,  # elapsed duration (micros)
                "c": self.aborted_span.category,  # category code
            }

        out = {
            "t": self.total_elapsed_micros,  # total elapsed microseconds
            "a": 1 if self.was_aborted else 0,  # was aborted flag (1 or 0)
            "ab": aborted_dict,  # aborted span details or None
            "top": [
                {
                    "l": _clean_label(item.label),  # label
                    "d": _to_micros(item.duration),  # duration (micros)
                    "c": _CATEGORY_CODE_MAP.get(
                        item.category, "O"
                    )  # category code
                    if item.category
                    else "O",
                }
                for item in self.top_n
            ],
            "sum": {
                _CATEGORY_CODE_MAP.get(cat, "O"): {  # category code key
                    "s": _to_micros(data.sum),  # sum of durations (micros)
                    "c": data.count,  # count of events
                    "m": _to_micros(data.mean),  # mean duration (micros)
                    "sd": _to_micros(data.std),  # standard deviation (micros)
                }
                for cat, data in self.summary.items()
            },
            "sel": self.selection,  # canonical selection string
        }
        return out


def format_selection_string(
    flags_dict: dict[str, typing.Any] | None,
) -> str:
    """Reconstruct a canonical selection string from parsed flags.

    Args:
        flags_dict: Serialized flags dictionary from parse_flags event.

    Returns:
        Canonical selection string (capped at 1000 characters).
        Returns empty string if no selection was specified.
    """
    if flags_dict is None:
        return ""

    raw_selection = flags_dict.get("selection") or []
    run_affected = flags_dict.get("run_affected_tests", False)

    selection_parts: list[str] = []
    if raw_selection:
        try:
            match_groups = selection._parse_selection_command_line(
                raw_selection
            )
            selection_parts.extend(str(group) for group in match_groups)
        except Exception:
            selection_parts.extend(raw_selection)

    if run_affected:
        selection_parts.append("--run-affected-tests")

    return " ".join(selection_parts)[:1000]


def _format_top_n_event_label(payload: event.EventPayloadUnion | None) -> str:
    if payload is not None and payload.test_suite_started is not None:
        return f"Running TestSuite {payload.test_suite_started.name}"
    return str(payload)


def compute_stats(log_source: LogSource) -> ExecutionStats:
    """Calculate and return statistics from the log source.

    This function processes the event stream to compute durations for various operations.
    It filters out container events (like groups) and child events whose parents are
    already accounted for to provide a meaningful summary of where time was spent.

    Args:
        log_source (LogSource): The source of log events to analyze.

    Returns:
        ExecutionStats: A dataclass containing top N events and summary stats.
    """
    event_dict: dict[event.Id, event.EventSpan] = dict()
    global_start_time: float | None = None
    global_end_time: float | None = None
    last_event_time: float = 0.0
    first_event_time: float | None = None
    aborted_suite_id: event.Id | None = None
    aborted_suite_time: float | None = None
    parsed_flags: dict[str, typing.Any] | None = None

    # Fast-path: Only parse starting, ending, and parse_flags events, skipping
    # JSON decoding and dataclass deserialization for all other events
    # (such as program_output lines).
    for element in log_source.read_log(
        event_filter=lambda d: bool(
            d.get("starting")
            or d.get("ending")
            or (
                isinstance(d.get("payload"), dict)
                and "parse_flags" in d["payload"]
            )
        ),
        line_filter=lambda l: '"starting"' in l
        or '"ending"' in l
        or '"parse_flags"' in l,
    ):
        if element.log_event is None or element.log_event.id is None:
            continue
        event_id = element.log_event.id
        ts = element.log_event.timestamp
        if first_event_time is None:
            first_event_time = ts
        last_event_time = max(last_event_time, ts)

        payload = element.log_event.payload
        if payload is not None and payload.parse_flags is not None:
            parsed_flags = payload.parse_flags

        if event_id == event.GLOBAL_RUN_ID:
            if element.log_event.starting:
                global_start_time = ts
            elif element.log_event.ending:
                global_end_time = ts
            continue

        if element.log_event.starting:
            event_dict[event_id] = event.EventSpan(
                start_time=ts,
                start_event=element.log_event,
            )
        elif element.log_event.ending and event_id in event_dict:
            payload = element.log_event.payload
            if (
                payload is not None
                and payload.test_suite_ended is not None
                and payload.test_suite_ended.status
                == event.TestSuiteStatus.ABORTED
            ):
                aborted_suite_id = event_id
                aborted_suite_time = ts
            else:
                event_dict[event_id].duration = (
                    ts - event_dict[event_id].start_time
                )

    # In-flight / open spans
    open_spans = [
        span
        for span in event_dict.values()
        if span.duration is None
        and span.category != event.EventStatCategory.IGNORE
    ]
    open_span_ids = {
        span.start_event.id
        for span in open_spans
        if span.start_event.id is not None
    }

    # Drop Ignore and incomplete events
    filtered_events = {
        event_id: span
        for event_id, span in event_dict.items()
        if span.duration is not None
        and span.category != event.EventStatCategory.IGNORE
    }

    # Drop children of existing parents (or children of open parents)
    final_events = [
        span
        for span in filtered_events.values()
        if span.start_event.parent not in filtered_events
        and span.start_event.parent not in open_span_ids
    ]

    categorized_events: dict[
        event.EventStatCategory, list[event.EventSpan]
    ] = collections.defaultdict(list)
    for span in final_events:
        if span.category:
            categorized_events[span.category].append(span)

    filtered_and_sorted_events = sorted(
        [
            e
            for e in final_events
            if e.duration is not None
            and e.duration >= _SUMMARY_TOP_N_CUTOFF_SECONDS
        ],
        key=lambda x: x.duration or 0.0,
        reverse=True,
    )

    top_n_list: list[TopNEvent] = []
    for event_stat in filtered_and_sorted_events[:_SUMMARY_TOP_N_MAX_COUNT]:
        top_n_list.append(
            TopNEvent(
                label=_format_top_n_event_label(event_stat.start_event.payload),
                duration=event_stat.duration or 0.0,
                category=event_stat.category,
            )
        )

    summary_dict: dict[event.EventStatCategory, CategoryStats] = {}
    for category, events in categorized_events.items():
        durations = [x.duration for x in events if x.duration is not None]
        count = len(durations)
        if count == 0:
            continue
        total_duration = sum(durations)
        mean = total_duration / count
        std = statistics.stdev(durations) if count > 1 else 0.0

        summary_dict[category] = CategoryStats(
            sum=total_duration, count=count, mean=mean, std=std
        )

    # Identify aborted in-flight span
    top_open_spans = [
        span
        for span in open_spans
        if span.start_event.parent not in open_span_ids
    ]

    abort_timestamp = (
        aborted_suite_time
        if aborted_suite_time is not None
        else (
            global_end_time if global_end_time is not None else last_event_time
        )
    )

    aborted_span: AbortedSpan | None = None
    if top_open_spans:
        chosen_span = None
        if aborted_suite_id is not None and aborted_suite_id in event_dict:
            chosen_span = event_dict[aborted_suite_id]
        if chosen_span is None:
            chosen_span = max(
                top_open_spans,
                key=lambda s: max(0.0, abort_timestamp - s.start_time),
            )
        elapsed = max(0.0, abort_timestamp - chosen_span.start_time)
        cat_code = (
            _CATEGORY_CODE_MAP.get(chosen_span.category, "O")
            if chosen_span.category
            else "O"
        )
        aborted_span = AbortedSpan(
            label=_format_top_n_event_label(chosen_span.start_event.payload),
            elapsed_micros=_to_micros(elapsed),
            category=cat_code,
        )

    was_aborted = (
        aborted_span is not None
        or aborted_suite_id is not None
        or (global_start_time is not None and global_end_time is None)
    )

    if global_start_time is not None:
        end_t = (
            global_end_time if global_end_time is not None else last_event_time
        )
        total_elapsed_micros = _to_micros(max(0.0, end_t - global_start_time))
    elif first_event_time is not None:
        total_elapsed_micros = _to_micros(
            max(0.0, last_event_time - first_event_time)
        )
    else:
        total_elapsed_micros = 0

    return ExecutionStats(
        top_n=top_n_list,
        summary=summary_dict,
        was_aborted=was_aborted,
        total_elapsed_micros=total_elapsed_micros,
        aborted_span=aborted_span,
        selection=format_selection_string(parsed_flags),
    )
