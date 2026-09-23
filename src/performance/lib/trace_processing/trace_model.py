# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Trace model data structures."""

import copy
import enum
from collections.abc import Mapping, Sequence
from functools import total_ordering
from typing import Any, Iterator, Self, TypeVar

from trace_processing import trace_time

INT32_MIN = -0x80000000


class InstantEventScope(enum.Enum):
    """Scope within a trace that an `InstantEvent` applies to."""

    THREAD = enum.auto()
    PROCESS = enum.auto()
    GLOBAL = enum.auto()


class FlowEventPhase(enum.Enum):
    """Phase within the control flow lifecycle that a `FlowEvent` applies to."""

    START = enum.auto()
    STEP = enum.auto()
    END = enum.auto()


class Event:
    """Base class for all trace events in a trace model.  Contains fields that
    are common to all trace event types.
    """

    def __init__(
        self,
        category: str,
        name: str,
        start: trace_time.TimePoint,
        pid: int,
        tid: int,
        args: dict[str, Any] | None,
    ) -> None:
        self.category = category
        self.name = name
        self.start = start
        self.pid = pid
        self.tid = tid
        # Any extra arguments that the event contains.
        self._args = args if args else None

    @property
    def args(self) -> dict[str, Any]:
        return self._args if self._args else {}

    @args.setter
    def args(self, rhs: dict[str, Any]) -> None:
        self._args = rhs if rhs else None

    @classmethod
    def _consume_dict(cls, event_dict: dict[str, Any]) -> Self:
        """Factory-style constructor. Intended for use by subclasses."""
        category: str = event_dict["cat"]
        name: str = event_dict["name"]
        start: trace_time.TimePoint = trace_time.TimePoint.from_epoch_delta(
            trace_time.TimeDelta.from_microseconds(event_dict["ts"])
        )
        pid: int = event_dict["pid"]
        tid: int = event_dict["tid"]
        args: dict[str, Any] = event_dict.get("args", {})

        del event_dict  # So that we get ownership of `args`.
        return cls(category, name, start, pid, tid, args)

    def end_time(self) -> trace_time.TimePoint | None:
        """Get the ending time for this Event

        Returns:
           * The end time, if the Event has a `duration`
           * None otherwise
        """
        return None


class InstantEvent(Event):
    """An event that corresponds to a single moment in time."""

    INSTANT_EVENT_SCOPE_MAP: dict[str, InstantEventScope] = {
        "g": InstantEventScope.GLOBAL,
        "p": InstantEventScope.PROCESS,
        "t": InstantEventScope.THREAD,
    }

    def __init__(self, scope: InstantEventScope, base: Event) -> None:
        super().__init__(
            base.category, base.name, base.start, base.pid, base.tid, base.args
        )
        self.scope: InstantEventScope = scope

    @classmethod
    def consume_dict(cls, event_dict: dict[str, Any]) -> Self:
        """Factory-style constructor."""
        scope_key: str = "s"
        if scope_key not in event_dict:
            raise TypeError(
                f"Expected dictionary to have a field '{scope_key}' of type "
                f"str: {event_dict}"
            )
        if not isinstance(event_dict[scope_key], str):
            raise TypeError(
                f"Expected dictionary to have a field '{scope_key}' of type "
                f"str: {event_dict}"
            )
        scope_str: str = event_dict[scope_key]
        if scope_str not in InstantEvent.INSTANT_EVENT_SCOPE_MAP:
            raise TypeError(
                f"Expected '{scope_key}' (scope field) of dict to be one of "
                f"{list(InstantEvent.INSTANT_EVENT_SCOPE_MAP)}: {event_dict}"
            )
        scope: InstantEventScope = InstantEvent.INSTANT_EVENT_SCOPE_MAP[
            scope_str
        ]

        return cls(scope, base=Event._consume_dict(event_dict))


class CounterEvent(Event):
    """An event that tracks the count of some quantity."""

    def __init__(self, id: int | None, base: Event) -> None:
        super().__init__(
            base.category, base.name, base.start, base.pid, base.tid, base.args
        )
        self.id: int | None = id

    @classmethod
    def consume_dict(cls, event_dict: dict[str, Any]) -> Self:
        """Factory-style constructor."""
        id_key: str = "id"
        id: int | None = None
        if id_key in event_dict:
            try:
                id = int(event_dict[id_key], 0)
            except (TypeError, ValueError) as t:
                raise TypeError(
                    f"Expected '{id_key}' field to be an int or a string that "
                    f"parses as int: {event_dict}"
                ) from t

        return cls(id, base=Event._consume_dict(event_dict))


class DurationEvent(Event):
    """An event which describes work that is happening synchronously on one
    thread.

    In the Fuchsia trace model, matching begin/end duration in the raw Chrome
    trace format are merged into a single `DurationEvent`. Chrome complete
    events become `DurationEvent`s as well. Dangling Chrome begin/end events
    (i.e. they don't have a matching end/begin event) are dropped.
    """

    duration: trace_time.TimeDelta | None
    parent: Self | None
    child_durations: list[Self]
    child_flows: list["FlowEvent"]

    def __init__(
        self,
        duration: trace_time.TimeDelta | None,
        parent: Self | None,
        child_durations: list[Self],
        child_flows: list["FlowEvent"],
        base: Event,
    ) -> None:
        super().__init__(
            base.category, base.name, base.start, base.pid, base.tid, base.args
        )
        self.duration = duration
        self.parent = parent
        self.child_durations = child_durations
        self.child_flows = child_flows

    @classmethod
    def consume_dict(cls, event_dict: dict[str, Any]) -> Self:
        """Factory-style constructor."""
        duration_key: str = "dur"
        duration: trace_time.TimeDelta | None = None
        microseconds: float | int | None = event_dict.get(duration_key, None)
        if microseconds is not None:
            if not isinstance(microseconds, (int, float)):
                raise TypeError(
                    f"Expected dictionary to have a field '{duration_key}' of "
                    f"type float or int: {event_dict}"
                )
            duration = trace_time.TimeDelta.from_microseconds(
                float(microseconds)
            )

        return cls(
            duration=duration,
            parent=None,
            child_durations=[],
            child_flows=[],
            base=Event._consume_dict(event_dict),
        )

    def end_time(self) -> trace_time.TimePoint | None:
        if self.duration:
            return self.start + self.duration
        else:
            return None


class AsyncEvent(Event):
    """An event which describes work which is happening asynchronously and which
    may span multiple threads.

    Dangling Chrome async begin/end events are dropped.
    """

    def __init__(
        self, id: int, duration: trace_time.TimeDelta | None, base: Event
    ) -> None:
        super().__init__(
            base.category, base.name, base.start, base.pid, base.tid, base.args
        )
        self.id: int = id
        self.duration: trace_time.TimeDelta | None = duration

    @classmethod
    def consume_dict(cls, id: int, event_dict: dict[str, Any]) -> Self:
        """Factory-style constructor."""
        return cls(id, duration=None, base=Event._consume_dict(event_dict))

    def end_time(self) -> trace_time.TimePoint | None:
        if self.duration:
            return self.start + self.duration
        else:
            return None


class FlowEvent(Event):
    """An event which describes control flow handoffs between threads or across
    processes.

    Malformed flow events are dropped.  Malformed flow events could be any of:
      * A begin flow event with a (category, name, id) tuple already in progress
        (this is uncommon in practice).
      * A step flow event with no preceding (category, name, id) tuple.
      * An end flow event with no preceding (category, name, id) tuple.
    """

    FLOW_EVENT_PHASE_MAP: dict[str, FlowEventPhase] = {
        "s": FlowEventPhase.START,
        "t": FlowEventPhase.STEP,
        "f": FlowEventPhase.END,
    }

    def __init__(
        self,
        id: str,
        phase: FlowEventPhase,
        enclosing_duration: DurationEvent | None,
        previous_flow: Self | None,
        next_flow: Self | None,
        base: Event,
    ) -> None:
        super().__init__(
            base.category, base.name, base.start, base.pid, base.tid, base.args
        )
        self.id: str = id
        self.phase: FlowEventPhase = phase
        self.enclosing_duration: DurationEvent | None = enclosing_duration
        self.previous_flow: Self | None = previous_flow
        self.next_flow: Self | None = next_flow

    @classmethod
    def consume_dict(
        cls,
        id: str,
        enclosing_duration: DurationEvent | None,
        event_dict: dict[str, Any],
    ) -> Self:
        """Factory-style constructor."""
        phase_key: str = "ph"
        if phase_key not in event_dict:
            raise TypeError(
                f"Expected dictionary to have a field '{phase_key}' of type "
                f"str: {event_dict}"
            )
        if not isinstance(event_dict[phase_key], str):
            raise TypeError(
                f"Expected dictionary to have a field '{phase_key}' of type "
                f"str: {event_dict}"
            )
        phase_str: str = event_dict[phase_key]
        if phase_str not in FlowEvent.FLOW_EVENT_PHASE_MAP:
            raise TypeError(
                f"Expected '{phase_key}' (phase field) of dict to be one of "
                f"{list(FlowEvent.FLOW_EVENT_PHASE_MAP)}: {event_dict}"
            )
        phase: FlowEventPhase = FlowEvent.FLOW_EVENT_PHASE_MAP[phase_str]

        return cls(
            id,
            phase,
            enclosing_duration,
            previous_flow=None,
            next_flow=None,
            base=Event._consume_dict(event_dict),
        )


@total_ordering
class ThreadState(enum.Enum):
    """Phase within the control flow lifecycle that a `FlowEvent` applies to."""

    # Basic thread states, in zx_info_thread_t.state.
    ZX_THREAD_STATE_NEW = 0x0000
    ZX_THREAD_STATE_RUNNING = 0x0001
    ZX_THREAD_STATE_SUSPENDED = 0x0002
    # BLOCKED is never returned by itself.
    # It is always returned with a more precise reason.
    # See below.
    ZX_THREAD_STATE_BLOCKED = 0x0003
    ZX_THREAD_STATE_DYING = 0x0004
    ZX_THREAD_STATE_DEAD = 0x0005

    # More precise thread states.
    ZX_THREAD_STATE_BLOCKED_EXCEPTION = 0x0103
    ZX_THREAD_STATE_BLOCKED_SLEEPING = 0x0203
    ZX_THREAD_STATE_BLOCKED_FUTEX = 0x0303
    ZX_THREAD_STATE_BLOCKED_PORT = 0x0403
    ZX_THREAD_STATE_BLOCKED_CHANNEL = 0x0503
    ZX_THREAD_STATE_BLOCKED_WAIT_ONE = 0x0603
    ZX_THREAD_STATE_BLOCKED_WAIT_MANY = 0x0703
    ZX_THREAD_STATE_BLOCKED_INTERRUPT = 0x0803
    ZX_THREAD_STATE_BLOCKED_PAGER = 0x0903

    def __lt__(self, other: Any) -> bool:
        if isinstance(other, ThreadState):
            return self.value < other.value
        return NotImplemented


class SchedulingRecord:
    """A record giving us information about cpu scheduling decisions"""

    def __init__(
        self,
        start: trace_time.TimePoint,
        tid: int,
        prio: int | None,
        args: dict[str, Any],
    ) -> None:
        self.start: trace_time.TimePoint = start
        self.tid: int = tid
        self.prio: int | None = prio
        self.args: dict[str, Any] = args.copy()

    def is_idle(self) -> bool:
        """
        True if the incoming thread is the idle thread
        """
        return self.prio == INT32_MIN


class ContextSwitch(SchedulingRecord):
    """A record indicating that a thread has been scheduled on a given cpu"""

    def __init__(
        self,
        start: trace_time.TimePoint,
        incoming_tid: int,
        outgoing_tid: int,
        incoming_prio: int | None,
        outgoing_prio: int | None,
        outgoing_state: ThreadState,
        args: dict[str, Any],
    ):
        super().__init__(start, incoming_tid, incoming_prio, args.copy())
        self.outgoing_tid = outgoing_tid
        self.outgoing_prio = outgoing_prio
        self.outgoing_state = outgoing_state


class Waking(SchedulingRecord):
    """A record indicating that a thread has been unblocked and is waiting to run on a given cpu"""

    def __init__(
        self,
        start: trace_time.TimePoint,
        tid: int,
        prio: int | None,
        args: dict[str, Any],
    ) -> None:
        super().__init__(start, tid, prio, args.copy())


class Thread:
    """A thread within a trace model."""

    def __init__(
        self,
        tid: int,
        name: str | None = None,
        events: list[Event] | None = None,
    ) -> None:
        self.tid: int = tid
        self.name: str = "" if name is None else name
        self.events: list[Event] = [] if events is None else events


class Process:
    """A process within a trace model."""

    def __init__(
        self,
        pid: int,
        name: str | None = None,
        threads: list[Thread] | None = None,
    ) -> None:
        self.pid: int = pid
        self.name: str = "" if name is None else name
        self.threads: list[Thread] = [] if threads is None else threads


class Model:
    """The root of the trace model."""

    def __init__(
        self,
        processes: Sequence[Process],
        scheduling_records: Mapping[int, Sequence[SchedulingRecord]],
    ):
        self.processes: Sequence[Process] = processes
        self.scheduling_records: Mapping[
            int, Sequence[SchedulingRecord]
        ] = scheduling_records

    def all_events(self) -> Iterator[Event]:
        for process in self.processes:
            for thread in process.threads:
                for event in thread.events:
                    yield event

    def slice(
        self,
        start: trace_time.TimePoint | None = None,
        end: trace_time.TimePoint | None = None,
    ) -> Self:
        """Extract a sub-Model defined by a time interval.

        Args:
            start: Start of the time interval.  If None, the start time of the
                source Model is used.  Only trace events that begin at or after
                `start` will be included in the sub-model.
            end: End of the time interval.  If None, the end time of the source
                Model is used.  Only trace events that end at or before `end`
                will be included in the sub-model.
        """
        # The various event types have references to other events, which we will
        # need to update so that all the relations in the new model stay within
        # the new model. This dict tracks for each event of the old model, which
        # event in the new model it corresponds to.
        new_event_map: dict[Any, Event] = {}

        T = TypeVar("T")

        def get_new_event(old_event: T) -> T | None:
            new_event: Event | None = new_event_map.get(old_event)
            if not new_event:
                return None
            assert isinstance(new_event, type(old_event))
            return new_event

        # Step 1: Populate the model with new event objects. These events will
        # have references into the old model.
        new_processes = []
        for process in self.processes:
            new_process = Process(process.pid, process.name)
            new_processes.append(new_process)
            for thread in process.threads:
                new_thread = Thread(thread.tid, thread.name)
                new_process.threads.append(new_thread)
                for event in thread.events:
                    # Exclude any event that starts or ends outside of the
                    # specified range.
                    if (start is not None and event.start < start) or (
                        end is not None and event.start > end
                    ):
                        continue

                    # Exclude any event whose duration ends outside of the
                    # specified range.
                    if isinstance(event, (AsyncEvent, DurationEvent)):
                        if (
                            end is not None
                            and event.duration > end - event.start
                        ):
                            continue

                    new_event = copy.copy(event)
                    new_thread.events.append(new_event)
                    new_event.args = event.args.copy()
                    new_event_map[event] = new_event

        # Step 2: Replace all referenced events by their corresponding ones in
        # the new model.
        for process in new_processes:
            for thread in process.threads:
                for event in thread.events:
                    if isinstance(event, DurationEvent):
                        if event.parent:
                            event.parent = get_new_event(event.parent)

                        updated_child_durations = []
                        for de in event.child_durations:
                            new_duration = get_new_event(de)
                            if new_duration is not None:
                                updated_child_durations.append(new_duration)
                        event.child_durations = updated_child_durations

                        updated_child_flows = []
                        for fe in event.child_flows:
                            new_flow = get_new_event(fe)
                            if new_flow is not None:
                                updated_child_flows.append(new_flow)
                        event.child_flows = updated_child_flows

                    elif isinstance(event, FlowEvent):
                        event.enclosing_duration = get_new_event(
                            event.enclosing_duration
                        )
                        event.previous_flow = get_new_event(event.previous_flow)
                        event.next_flow = get_new_event(event.next_flow)

        def slice_scheduling_records(record: SchedulingRecord) -> bool:
            return (start is None or record.start >= start) and (
                end is None or record.start <= end
            )

        new_scheduling_records = {
            cpu: list(filter(slice_scheduling_records, records))
            for cpu, records in self.scheduling_records.items()
        }
        return self.__class__(new_processes, new_scheduling_records)
