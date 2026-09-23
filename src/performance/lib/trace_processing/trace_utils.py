# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Utilities to filter and extract events and statistics from a trace Model."""

import itertools
import math
import statistics
from dataclasses import dataclass
from typing import (
    Any,
    Generator,
    Generic,
    Iterable,
    Iterator,
    List,
    Optional,
    Tuple,
    Type,
    TypeVar,
    overload,
)

from reporting import metrics
from trace_processing import trace_model, trace_time


# Compute the linear interpolated [percentile]th percentile
# (https://en.wikipedia.org/wiki/Percentile) of [values].
def percentile(values: Iterable[int | float], percentile: int) -> float:
    if not values:
        raise TypeError(
            "[values] must not be empty in order to compute percentile"
        )

    values_list: List[int | float] = sorted(values)
    if percentile == 100:
        return float(values_list[-1])

    index_as_float: float = float(len(values_list) - 1) * (0.01 * percentile)
    index: int = math.floor(index_as_float)
    if (index + 1) == len(values_list):
        return float(values_list[-1])
    index_fraction: float = index_as_float % 1
    return (
        values_list[index] * (1.0 - index_fraction)
        + values_list[index + 1] * index_fraction
    )


T = TypeVar("T", bound=trace_model.Event)


def filter_events(
    events: Iterable[trace_model.Event],
    type: type[T],
    category: Optional[str] = None,
    name: str | Iterable[str] | None = None,
) -> Generator[T, None, None]:
    """Filter |events| based on category, name, or type.

    Args:
      events: The set of events to filter.
      category: Category of events to include, or None to skip this filter.
      name: name or names of events to include, or None to skip this filter.
      type: Type of events to include. By default object to include all events.

    Returns:
      An [Iterator] of filtered events. Note that Iterators can only be iterated a single time, so
      the caller must create a local copy in order to loop over the filtered events more than once.
    """

    for event in events:
        category_matches: bool = category is None or event.category == category
        if name is None:
            name_matches = True
        elif isinstance(name, str):
            name_matches = event.name == name
        else:
            name_matches = event.name in name
        if isinstance(event, type) and category_matches and name_matches:
            yield event


T_Event = TypeVar("T_Event", bound=trace_model.Event)


@dataclass(frozen=True)
class EventFilter(Generic[T_Event]):
    """A filter for trace events."""

    type: Type[T_Event]
    category: Optional[str] = None
    name: Optional[str] = None


T1 = TypeVar("T1", bound=trace_model.Event)
T2 = TypeVar("T2", bound=trace_model.Event)
T3 = TypeVar("T3", bound=trace_model.Event)
T4 = TypeVar("T4", bound=trace_model.Event)
T5 = TypeVar("T5", bound=trace_model.Event)


@overload
def filter_events_parallel(
    events: Iterable[trace_model.Event],
    filters: Tuple[EventFilter[T1]],
) -> Tuple[Generator[T1, None, None]]:
    ...


@overload
def filter_events_parallel(
    events: Iterable[trace_model.Event],
    filters: Tuple[EventFilter[T1], EventFilter[T2]],
) -> Tuple[Generator[T1, None, None], Generator[T2, None, None]]:
    ...


@overload
def filter_events_parallel(
    events: Iterable[trace_model.Event],
    filters: Tuple[EventFilter[T1], EventFilter[T2], EventFilter[T3]],
) -> Tuple[
    Generator[T1, None, None],
    Generator[T2, None, None],
    Generator[T3, None, None],
]:
    ...


@overload
def filter_events_parallel(
    events: Iterable[trace_model.Event],
    filters: Tuple[
        EventFilter[T1], EventFilter[T2], EventFilter[T3], EventFilter[T4]
    ],
) -> Tuple[
    Generator[T1, None, None],
    Generator[T2, None, None],
    Generator[T3, None, None],
    Generator[T4, None, None],
]:
    ...


@overload
def filter_events_parallel(
    events: Iterable[trace_model.Event],
    filters: Tuple[
        EventFilter[T1],
        EventFilter[T2],
        EventFilter[T3],
        EventFilter[T4],
        EventFilter[T5],
    ],
) -> Tuple[
    Generator[T1, None, None],
    Generator[T2, None, None],
    Generator[T3, None, None],
    Generator[T4, None, None],
    Generator[T5, None, None],
]:
    ...


def filter_events_parallel(
    events: Iterable[trace_model.Event],
    filters: Tuple[EventFilter[Any], ...],
) -> Tuple[Generator[trace_model.Event, None, None], ...]:
    """Filter |events| based on a list of filter tuples.

    This is more efficient than calling filter_events multiple times on the same
    iterable of events, especially if the iterable is a generator.

    Args:
      events: The set of events to filter.
      filters: A list of EventFilter objects.

    Returns:
      A tuple of [Generator]s of filtered events. Each generator corresponds
      to a filter in the |filters| list.
    """
    if not filters:
        return tuple()

    event_iters = itertools.tee(events, len(filters))

    results: List[Generator[trace_model.Event, None, None]] = []
    for i, f in enumerate(filters):
        results.append(
            filter_events(
                event_iters[i], type=f.type, category=f.category, name=f.name
            )
        )
    return tuple(results)


U = TypeVar("U", bound=trace_model.SchedulingRecord)


def filter_records(
    records: Iterable[trace_model.SchedulingRecord], type_: type[U]
) -> Iterator[U]:
    """Filters SchedulingRecords by type.

    Easier for mypy to grok than using types with filter().

    Args:
        records: Iterable of SchedulingRecords to be filtered.
        type_: The subclass of SchedulingRecord by which to filter.

    Yields:
        The elements of the iterable that are of the specified type.
    """
    for record in records:
        if isinstance(record, type_):
            yield record


def total_event_duration(
    events: Iterable[trace_model.Event],
) -> trace_time.TimeDelta:
    """Compute the total duration of all [Event]s in |events|.  This is the end
    of the last event minus the beginning of the first event.

    Args:
      events: The set of events to compute the duration for.

    Returns:
      Total event duration.
    """

    def event_times(
        e: trace_model.Event,
    ) -> Tuple[trace_time.TimePoint, trace_time.TimePoint]:
        end: trace_time.TimePoint = e.start
        if (
            isinstance(e, trace_model.AsyncEvent | trace_model.DurationEvent)
            and e.duration
        ):
            end = e.start + e.duration
        return (e.start, end)

    start_times, end_times = zip(*map(event_times, events))
    min_time: trace_time.TimePoint = min(
        start_times, default=trace_time.TimePoint.zero()
    )
    max_time: trace_time.TimePoint = max(
        end_times, default=trace_time.TimePoint.zero()
    )

    return max_time - min_time


def get_arg_values_from_events(
    events: Iterable[trace_model.Event],
    arg_key: str,
    arg_types: type | Tuple[type, ...] = object,
) -> Iterable[Any]:
    """Collect values from the |args| maps in |events|.

    Args:
      events: The events to collect args from.
      arg_key: The key in the |args| maps to collect values from.
      arg_type:

    Raises:
      KeyError: The value corresponding to |arg_key| was not present in one of
        the event's |args| map.

    Returns:
      An [Iterable] of collected values.
    """

    def event_to_arg_type(event: trace_model.Event) -> Any:
        has_arg: bool = arg_key in event.args
        arg: Any = event.args.get(arg_key, None)
        if not has_arg or not isinstance(arg, arg_types):
            raise KeyError(
                f"Error, expected events to include arg with key '{arg_key}' "
                f"of type(s) {str(arg_types)}"
            )
        return arg  # Cannot be None if we reach here

    return map(event_to_arg_type, events)


def get_following_flow_events(
    event: trace_model.Event,
) -> Iterable[trace_model.Event]:
    """Find all Events that are flow connected and follow |event|.

    Args:
      event: The starting event.

    Returns:
      An [Iterable] of flow connected events.
    """
    frontier = [event]
    visited: set[trace_model.Event] = set()

    def set_add(
        event_set: set[trace_model.Event], event: trace_model.Event
    ) -> bool:
        length_before = len(event_set)
        event_set.add(event)
        return len(event_set) != length_before

    while frontier:
        current: trace_model.Event = frontier.pop()
        added = set_add(visited, current)
        if not added:
            continue
        if isinstance(current, trace_model.DurationEvent):
            frontier.extend(current.child_durations)
            frontier.extend(current.child_flows)
        elif isinstance(current, trace_model.FlowEvent):
            if current.enclosing_duration:
                frontier.append(current.enclosing_duration)
            if current.next_flow:
                frontier.append(current.next_flow)

    def by_start_time(event: trace_model.Event) -> trace_time.TimePoint:
        return event.start

    for connected_event in sorted(visited, key=by_start_time):
        yield connected_event


def get_nearest_following_flow_event(
    event: trace_model.Event,
    following_event_category: str,
    following_event_name: str,
) -> trace_model.Event | None:
    """Find the nearest target event that is flow connected and follow |event|.

    Args:
      event: The starting event.
      following_event_category: Trace category of the target event.
      following_event_name: Trace event name of the target event.

    Returns:
      Flow connected events. If nothing is found, None.
    """
    filtered_following_events = filter_events(
        get_following_flow_events(event),
        category=following_event_category,
        name=following_event_name,
        type=trace_model.DurationEvent,
    )
    return next(iter(filtered_following_events), None)


# This method looks for a possible race between trace collection start in multiple processes.
#
# This problem usually occurs between Scenic, Magma and Display processes. The flow connection
# between these processes happens through koids of objects passed to Vulkan, which may be reused.
# When there is a gap in process trace collection starts, we may see the initial vsync event(s)
# connecting to multiple flows. We can safely skip these initial events. See
# https://fxbug.dev/322849857 for more detailed examples with screenshots.
#
# This method finds the first event flow the test interested in, finds processes in the flow, and
# cuts tracing when the latest process tracing started.
#
# This method used to check processes with given process and thread name. That did not work because
# different product may have different process setup, for example Display process with different
# names.
def adjust_to_common_process_start(
    model: trace_model.Model,
    name: str,
    type: type[T],
    category: Optional[str] = None,
) -> trace_model.Model:
    """Adjust model to a consistent start time tracking the latest first event recorded from a
    list of processes. The list of processes are selected through matching event flow.

    Args:
      model: Trace model.
      name: name of event, or None to skip this filter.
      category: Category of event, or None to skip this filter.
      type: Type of event. By default object to include all events.

    Returns:
      Model adjusted to a start time tracking the latest first event recorded from processes from
      the given event flow.

    Raises:
      KeyError if no matched event is found.
    """

    filtered = filter_events(
        model.all_events(), category=category, name=name, type=type
    )
    # Get the first matched event.
    begin_event = next(filtered, None)

    if begin_event is None:
        raise KeyError(f"Error, expected event with name '{name}'")

    events_in_flow: Iterable[trace_model.Event] = get_following_flow_events(
        begin_event
    )

    process_ids = set()
    process_ids.add(begin_event.pid)
    for event in events_in_flow:
        process_ids.add(event.pid)

    process_matches = [p for p in model.processes if p.pid in process_ids]
    consistent_start_time = max(
        min(
            (
                event.start
                for thread in process.threads
                for event in thread.events
            ),
            default=trace_time.TimePoint(),
        )
        for process in process_matches
    )
    return model.slice(start=consistent_start_time)


def standard_metrics_set(
    values: List[int | float],
    label_prefix: str,
    unit: metrics.Unit,
    percentiles: tuple[int, int, int, int, int] = (5, 25, 50, 75, 95),
    durations: List[float] | None = None,
    doc_prefix: str | None = None,
) -> list[metrics.TestCaseResult]:
    """Generates min, max, average and percentiles metrics for the given values.

    Args:
        values: Input to create the metrics from.
        label_prefix: metric labels will be '{label_prefix}Min', '{label_prefix}Max',
            '{label_prefix}Average' and '{label_prefix}P*' (percentiles).
        unit: The metrics unit.
        percentiles: Percentiles to output.
        durations: Optional. If passed in, the average percentage is calculated
            proportional to the duration of each Event's CPU usage, instead of
            just taking the average of all the values. The latter assumes that
            each value had the same duration (e.g. 1 s). For example:
                cpu_percentages: [30, 2]
                duration: [1, 5]
            The average would be around (30*1+2*5)/(1+5) = 6.7%
            instead of statistics.mean([30, 1]) = 15.5%
        doc_prefix: a descriptive string with which to document this set of metrics.
            Defaults to label_prefix.

    Returns:
        A list of TestCaseResults representing each of the generated metrics.
    """
    doc_prefix = doc_prefix if doc_prefix else label_prefix

    results = [
        metrics.TestCaseResult(
            f"{label_prefix}P{p}",
            unit,
            [percentile(values, p)],
            f"{doc_prefix}, {p}th percentile",
        )
        for p in percentiles
    ]

    results += [
        metrics.TestCaseResult(
            f"{label_prefix}Min",
            unit,
            [min(values)],
            f"{doc_prefix}, minimum",
        ),
        metrics.TestCaseResult(
            f"{label_prefix}Max",
            unit,
            [max(values)],
            f"{doc_prefix}, maximum",
        ),
    ]

    average: float = 0
    if durations is not None:
        proportional_percentages: list[float] = [
            duration * values[i] for i, duration in enumerate(durations)
        ]
        # The total average CPU percentage is:
        # (sum of the average_percentages) / (total trace duration)
        average_percentage = sum(proportional_percentages) / sum(durations)
        average = average_percentage
    else:
        average = statistics.mean(values)
    results += [
        metrics.TestCaseResult(
            f"{label_prefix}Average",
            unit,
            [average],
            f"{doc_prefix}, mean",
        ),
    ]

    return results
