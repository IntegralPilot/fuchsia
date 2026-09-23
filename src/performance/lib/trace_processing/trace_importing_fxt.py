# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Isolated Perfetto FXT direct ingestion and parsing backend.

This module handles the direct ingestion of Fuchsia Trace (FXT) files into the
in-memory trace model using PerfettoTraceProcessor.
"""

import dataclasses
from collections import defaultdict
from collections.abc import Set
from typing import Any, NamedTuple

from tp_shell import PerfettoTraceProcessor
from trace_processing import trace_model, trace_time
from trace_processing.trace_importing import construct_model


def create_model_from_tp_session(
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

    importer = _FxtImporter(session, patterns, categories)
    processes = importer.get_pid_to_name()
    threads = importer.get_threads()
    events = importer.get_events()
    scheduling = importer.get_scheduling_records()

    return construct_model(
        processes.pid_to_name,
        threads.tid_to_name,
        threads.tid_to_pid,
        events.events,
        scheduling.records,
    )


def create_model_from_fxt_path_directly(
    trace_path: str,
    patterns: Set[str] | None = None,
    categories: Set[str] | None = None,
    trace_processor_shell_path: str | None = None,
) -> trace_model.Model:
    """Create a trace model directly from a Perfetto trace file path.

    A temporary PerfettoTraceProcessor session is created for the duration of model creation
    and closed when model creation completes.

    Args:
        trace_path: Path to trace file or URL string.
        patterns: Optional regex patterns to filter events.
        categories: Optional categories to filter events.
        trace_processor_shell_path: Optional path to Perfetto trace_processor_shell executable.

    Returns:
        A trace model containing the trace data.
    """
    with PerfettoTraceProcessor(
        trace_path=trace_path, tp_shell_path=trace_processor_shell_path
    ) as session:
        return create_model_from_tp_session(session, patterns, categories)


class _FxtImporter:
    """Encapsulates Perfetto FXT ingestion session state and query parsing."""

    class _Processes(NamedTuple):
        pid_to_name: dict[int, str]

    class _Threads(NamedTuple):
        tid_to_name: dict[int, str]
        tid_to_pid: dict[int, int]

    class _Slices(NamedTuple):
        events: list[trace_model.Event]
        id_to_event: dict[int, trace_model.Event]
        parent_map: dict[int, int]

    class _Counters(NamedTuple):
        events: list[trace_model.Event]

    class _Flows(NamedTuple):
        events: list[trace_model.Event]

    class _SchedulingRecords(NamedTuple):
        records: dict[int, list[trace_model.SchedulingRecord]]

    class _Events(NamedTuple):
        events: list[trace_model.Event]

    def __init__(
        self,
        session: PerfettoTraceProcessor,
        patterns: Set[str] | None = None,
        categories: Set[str] | None = None,
    ) -> None:
        self.session = session
        self.patterns = patterns
        self.categories = categories
        self._args_map: dict[int, dict[str, Any]] | None = None

    def _query(self, sql: str) -> list[list[str]]:
        """Executes a SQL query and returns results as lists of string rows."""
        results = self.session.run_query(sql)
        if not results:
            return []
        keys = list(results[0].keys())
        return [
            [str(r[k]) if r[k] is not None else "" for k in keys]
            for r in results
        ]

    def get_pid_to_name(self) -> _Processes:
        sql = (
            "SELECT DISTINCT pid, COALESCE(name, '') as name FROM process ORDER"
            " BY name ASC"
        )
        data = self._query(sql)
        return self._Processes({int(row[0]): row[1] for row in data})

    def get_threads(self) -> _Threads:
        sql = """
        SELECT
          tid,
          process.pid,
          COALESCE(thread.name, '') as name
        FROM thread
        JOIN process USING (upid)
        ORDER BY name ASC
        """
        data = self._query(sql)
        raw_tid_to_name: dict[int, str] = {}
        raw_tid_to_pid: dict[int, int] = {}
        for row in data:
            tid = int(row[0])
            raw_tid_to_name[tid] = row[2]
            raw_tid_to_pid[tid] = int(row[1])

        tid_to_name = {k: v for k, v in raw_tid_to_name.items() if v}
        tid_to_pid = {k: v for k, v in raw_tid_to_pid.items() if k != v}
        return self._Threads(tid_to_name, tid_to_pid)

    def _get_args_map(self) -> dict[int, dict[str, Any]]:
        if self._args_map is not None:
            return self._args_map

        sql = """
        SELECT
          arg_set_id,
          key,
          COALESCE(int_value, ''),
          COALESCE(real_value, ''),
          COALESCE(string_value, '')
        FROM args
        """
        data = self._query(sql)
        arg_set_id_to_args: dict[int, dict[str, Any]] = defaultdict(dict)
        for row in data:
            arg_id_str = row[0]
            if arg_id_str:
                arg_id = int(arg_id_str)
                key = row[1]
                int_val_str = row[2]
                real_val_str = row[3]
                str_val = row[4]
                val: Any
                if int_val_str != "":
                    val = int(int_val_str)
                elif real_val_str != "":
                    val = float(real_val_str)
                else:
                    val = str_val
                arg_set_id_to_args[arg_id][key] = val
        self._args_map = arg_set_id_to_args
        return arg_set_id_to_args

    def get_slices(self) -> _Slices:
        arg_set_id_to_args = self._get_args_map()
        where_clauses: list[str] = []
        if self.categories:
            cats_joined = ", ".join(f"'{c}'" for c in self.categories)
            where_clauses.append(
                f"COALESCE(slice.category, '') IN ({cats_joined})"
            )
        if self.patterns is not None and len(self.patterns) > 0:
            combined_pattern = "|".join(f"({p})" for p in self.patterns)
            where_clauses.append(f"slice.name REGEXP '{combined_pattern}'")

        if where_clauses:
            where_sql = " WHERE " + " OR ".join(where_clauses)
        elif self.patterns is not None and len(self.patterns) == 0:
            where_sql = " WHERE 0"
        else:
            where_sql = ""

        sql = f"""
        SELECT
          slice.id,
          COALESCE(slice.parent_id, ''),
          COALESCE(slice.arg_set_id, ''),
          COALESCE(slice.ts, 0),
          COALESCE(slice.dur, 0),
          COALESCE(slice.category, ''),
          COALESCE(slice.name, ''),
          COALESCE(thread.tid, 0),
          COALESCE(process.pid, async_process.pid, 0),
          (thread_track.id IS NULL) AS is_async
        FROM slice
        LEFT JOIN track ON slice.track_id = track.id
        LEFT JOIN thread_track ON track.id = thread_track.id
        LEFT JOIN thread ON thread_track.utid = thread.utid
        LEFT JOIN process_track ON track.id = process_track.id
        LEFT JOIN process ON COALESCE(thread.upid, process_track.upid) = process.upid
        LEFT JOIN args ON track.source_arg_set_id = args.arg_set_id AND args.key = 'upid'
        LEFT JOIN process async_process ON args.int_value = async_process.upid
        {where_sql}
        """
        data = self._query(sql)
        slice_events: list[trace_model.Event] = []
        slice_id_to_event: dict[int, trace_model.Event] = {}
        slice_parent_map: dict[int, int] = {}

        for row in data:
            dur_ns = int(row[4])
            ts_ns = int(row[3])
            arg_id_str = row[2]
            arg_id = int(arg_id_str) if arg_id_str else None
            args = (
                arg_set_id_to_args[arg_id]
                if arg_id is not None and arg_id in arg_set_id_to_args
                else {}
            )
            pid = int(row[8])
            tid = int(row[7])
            is_async = int(row[9]) == 1

            base = trace_model.Event(
                category=row[5],
                name=row[6],
                start=trace_time.TimePoint.from_epoch_delta(
                    trace_time.TimeDelta.from_nanoseconds(float(ts_ns))
                ),
                pid=pid,
                tid=0 if is_async else tid,
                args=args,
            )
            dur_delta = trace_time.TimeDelta.from_nanoseconds(float(dur_ns))
            event: trace_model.Event
            if is_async:
                event = trace_model.AsyncEvent(
                    id=0,
                    duration=dur_delta,
                    base=base,
                )
            elif dur_ns == 0:
                event = trace_model.InstantEvent(
                    scope=trace_model.InstantEventScope.THREAD,
                    base=base,
                )
            else:
                event = trace_model.DurationEvent(
                    duration=dur_delta,
                    parent=None,
                    child_durations=[],
                    child_flows=[],
                    base=base,
                )
            slice_events.append(event)
            slice_id = int(row[0])
            slice_id_to_event[slice_id] = event
            if row[1] != "":
                slice_parent_map[slice_id] = int(row[1])

        return self._Slices(slice_events, slice_id_to_event, slice_parent_map)

    def get_counter_events(self) -> _Counters:
        @dataclasses.dataclass(frozen=True)
        class _CounterKey:
            ts_ns: int
            pid: int
            tid: int
            event_name: str
            track_id: int | None

        @dataclasses.dataclass
        class _CounterGroup:
            name: str
            ts_ns: int
            pid: int
            tid: int
            id: int | None
            args: dict[str, Any]

        if self.patterns is not None and len(self.patterns) > 0:
            combined_pattern = "|".join(f"({p})" for p in self.patterns)
            where_sql = f" WHERE counter_track.name REGEXP '{combined_pattern}'"
        elif self.patterns is not None and len(self.patterns) == 0:
            where_sql = " WHERE 0"
        else:
            where_sql = ""

        sql = f"""
        SELECT
          counter.ts,
          counter.value,
          counter_track.name,
          COALESCE(process.pid, 0),
          COALESCE(thread.tid, 0),
          counter.arg_set_id,
          counter.id,
          counter_track.id
        FROM counter
        JOIN counter_track ON counter.track_id = counter_track.id
        LEFT JOIN process_counter_track ON counter_track.id = process_counter_track.id
        LEFT JOIN process ON process_counter_track.upid = process.upid
        LEFT JOIN thread_counter_track ON counter_track.id = thread_counter_track.id
        LEFT JOIN thread ON thread_counter_track.utid = thread.utid
        {where_sql}
        ORDER BY counter.ts ASC
        """
        data = self._query(sql)
        arg_set_id_to_args = self._get_args_map()
        counter_groups: dict[_CounterKey, _CounterGroup] = {}

        for row in data:
            ts_ns = int(row[0])
            val = float(row[1])
            track_name = row[2]
            event_name = track_name
            pid = int(row[3])
            tid = int(row[4])
            arg_id_str = row[5]
            arg_id = int(arg_id_str) if arg_id_str else None
            counter_id = int(row[6]) if len(row) > 6 and row[6] else None
            track_id = int(row[7]) if len(row) > 7 and row[7] else None

            args = (
                dict(arg_set_id_to_args[arg_id])
                if arg_id is not None and arg_id in arg_set_id_to_args
                else {}
            )

            # Fuchsia counter events ingested into Perfetto are named in counter_track
            # by joining category, counter metric name, and track ID with colons (':').
            # For example, a track name like "kmem_stats_a:zram_bytes:0" is created when a
            # counter event (such as category "kmem_stats_a" and name "zram_bytes") is
            # imported into Perfetto.
            # Splitting on colons extracts the base event name (e.g. "kmem_stats_a"), assigns
            # the counter value to the metric argument key (e.g. "zram_bytes"), and extracts
            # any track ID suffix.
            if ":" in track_name:
                parts = track_name.split(":")
                event_name = parts[0]
                arg_key = parts[1] if len(parts) > 1 else "value"
                args[arg_key] = val
                for part in parts[1:]:
                    try:
                        parsed_track_id = int(part)
                        if track_id is None:
                            track_id = parsed_track_id
                        break
                    except ValueError:
                        pass
            else:
                args["value"] = val

            key = _CounterKey(ts_ns, pid, tid, event_name, track_id)
            if key not in counter_groups:
                counter_groups[key] = _CounterGroup(
                    name=event_name,
                    ts_ns=ts_ns,
                    pid=pid,
                    tid=tid,
                    id=counter_id,
                    args=args,
                )
            else:
                counter_groups[key].args.update(args)

        counter_events: list[trace_model.Event] = []
        for group in counter_groups.values():
            base = trace_model.Event(
                category="",
                name=group.name,
                start=trace_time.TimePoint.from_epoch_delta(
                    trace_time.TimeDelta.from_nanoseconds(float(group.ts_ns))
                ),
                pid=group.pid,
                tid=group.tid,
                args=group.args,
            )
            counter_events.append(trace_model.CounterEvent(id=0, base=base))

        return self._Counters(counter_events)

    def get_flow_events(
        self,
        slice_id_to_event: dict[int, trace_model.Event],
        slice_parent_map: dict[int, int],
    ) -> _Flows:
        sql = """
        SELECT
          CAST(flow.slice_out AS TEXT),
          CAST(flow.slice_in AS TEXT),
          CAST(flow.trace_id AS TEXT)
        FROM flow
        WHERE flow.trace_id IS NOT NULL
          AND (flow.slice_out IS NOT NULL OR flow.slice_in IS NOT NULL)
        """
        data = self._query(sql)
        flow_events: list[trace_model.Event] = []
        key_to_flow_event: dict[tuple[int, int], trace_model.FlowEvent] = {}

        def get_or_create_flow_event(
            slice_id: int, flow_trace_id: int, is_out: bool
        ) -> trace_model.FlowEvent | None:
            """Retrieves or creates a FlowEvent for a (slice_id, flow_trace_id) pair.

            If the event already exists, updates its phase to STEP if it acts as both
            an incoming and outgoing node in the flow graph.
            """
            key = (slice_id, flow_trace_id)
            if key in key_to_flow_event:
                existing_flow_event = key_to_flow_event[key]
                # Upgrade phase to STEP if an END event is encountered as outgoing
                # or a START event is encountered as incoming.
                if (
                    is_out
                    and existing_flow_event.phase
                    == trace_model.FlowEventPhase.END
                ):
                    existing_flow_event.phase = trace_model.FlowEventPhase.STEP
                elif (
                    not is_out
                    and existing_flow_event.phase
                    == trace_model.FlowEventPhase.START
                ):
                    existing_flow_event.phase = trace_model.FlowEventPhase.STEP
                return existing_flow_event

            # Skip creation if the underlying slice was filtered out.
            base_event = slice_id_to_event.get(slice_id)
            if base_event is None:
                return None

            # Outgoing slices start as START; incoming slices start as END.
            phase = (
                trace_model.FlowEventPhase.START
                if is_out
                else trace_model.FlowEventPhase.END
            )

            # Associate FlowEvent with its enclosing DurationEvent. If the slice is an
            # InstantEvent (dur == 0) or AsyncEvent (unlikely), walk up the parent chain
            # to locate the enclosing DurationEvent.
            enclosing = None
            curr_id = slice_id
            event = slice_id_to_event.get(curr_id)
            if isinstance(event, trace_model.DurationEvent):
                enclosing = event
            elif slice_parent_map:
                while curr_id in slice_parent_map:
                    curr_id = slice_parent_map[curr_id]
                    event = slice_id_to_event.get(curr_id)
                    if isinstance(event, trace_model.DurationEvent):
                        enclosing = event
                        break

            flow_event = trace_model.FlowEvent(
                id=str(flow_trace_id),
                phase=phase,
                enclosing_duration=enclosing,
                previous_flow=None,
                next_flow=None,
                base=base_event,
            )
            if enclosing is not None:
                enclosing.child_flows.append(flow_event)

            key_to_flow_event[key] = flow_event
            flow_events.append(flow_event)
            return flow_event

        for row in data:
            raw_out = row[0]
            raw_in = row[1]
            flow_trace_id = int(row[2])

            s_out = int(raw_out) if raw_out else None
            s_in = int(raw_in) if raw_in else None

            flow_event_out = (
                get_or_create_flow_event(s_out, flow_trace_id, is_out=True)
                if s_out is not None
                else None
            )
            flow_event_in = (
                get_or_create_flow_event(s_in, flow_trace_id, is_out=False)
                if s_in is not None
                else None
            )

            if (
                flow_event_out is not None
                and flow_event_in is not None
                and s_out != s_in
            ):
                flow_event_out.next_flow = flow_event_in
                flow_event_in.previous_flow = flow_event_out

        return self._Flows(flow_events)

    def get_scheduling_records(
        self,
    ) -> _SchedulingRecords:
        sql = """
        WITH cs AS (
          SELECT
            s.ts AS ts,
            COALESCE(s.cpu, 0) AS cpu,
            COALESCE(s.dur, 0) AS dur,
            COALESCE(s.end_state, '') AS end_state,
            COALESCE(s.priority, 0) AS priority,
            COALESCE(t.tid, 0) AS tid,
            'Running' AS state
          FROM sched s
          LEFT JOIN thread t ON s.utid = t.utid
        ),
        waking AS (
          SELECT
            ts.ts AS ts,
            COALESCE(ts.cpu, ts.ucpu, 0) AS cpu,
            0 AS dur,
            '' AS end_state,
            0 AS priority,
            COALESCE(t.tid, 0) AS tid,
            'W' AS state
          FROM thread_state ts
          JOIN thread t ON ts.utid = t.utid
          WHERE ts.state = 'W'
        )
        SELECT ts, cpu, dur, end_state, priority, tid, state
        FROM (
          SELECT * FROM cs
          UNION ALL
          SELECT * FROM waking
          ORDER BY ts
        )
        """
        data = self._query(sql)
        scheduling_records: dict[
            int, list[trace_model.SchedulingRecord]
        ] = defaultdict(list)

        RUNNING_STATE = trace_model.ThreadState.ZX_THREAD_STATE_RUNNING
        BLOCKED_STATE = trace_model.ThreadState.ZX_THREAD_STATE_BLOCKED
        IDLE_TID = 0
        IDLE_PRIO = trace_model.INT32_MIN

        class CpuState(NamedTuple):
            last_end_ts: int | None
            last_tid: int
            last_prio: int | None
            last_end_state: str | None

        cpu_states: dict[int, CpuState] = {}

        for row in data:
            cpu = int(row[1])
            ts_ns = int(row[0])
            state = row[6]

            records = scheduling_records[cpu]
            timestamp = trace_time.TimePoint.from_epoch_delta(
                trace_time.TimeDelta.from_nanoseconds(float(ts_ns))
            )

            if state == "W":
                records.append(
                    trace_model.Waking(
                        start=timestamp,
                        tid=int(row[5]),
                        prio=0,
                        args={},
                    )
                )
                continue

            raw_dur = int(row[2])
            tid = int(row[5])
            prio = int(row[4])
            end_state_str = row[3] if row[3] else None

            cpu_state = cpu_states.get(
                cpu,
                CpuState(
                    last_end_ts=None,
                    last_tid=0,
                    last_prio=None,
                    last_end_state=None,
                ),
            )

            last_end_ts = cpu_state.last_end_ts
            last_tid = cpu_state.last_tid
            last_prio = cpu_state.last_prio
            last_end_state = cpu_state.last_end_state

            if last_end_ts is not None and ts_ns > last_end_ts:
                if last_tid != IDLE_TID:
                    outgoing_state = (
                        BLOCKED_STATE
                        if last_end_state in ("S", "D")
                        else RUNNING_STATE
                    )
                    gap_start_timestamp = trace_time.TimePoint.from_epoch_delta(
                        trace_time.TimeDelta.from_nanoseconds(
                            float(last_end_ts)
                        )
                    )
                    records.append(
                        trace_model.ContextSwitch(
                            start=gap_start_timestamp,
                            incoming_tid=IDLE_TID,
                            outgoing_tid=last_tid,
                            incoming_prio=IDLE_PRIO,
                            outgoing_prio=last_prio,
                            outgoing_state=outgoing_state,
                            args={},
                        )
                    )
                    last_tid = IDLE_TID
                    last_prio = IDLE_PRIO

                records.append(
                    trace_model.ContextSwitch(
                        start=timestamp,
                        incoming_tid=tid,
                        outgoing_tid=IDLE_TID,
                        incoming_prio=prio,
                        outgoing_prio=IDLE_PRIO,
                        outgoing_state=RUNNING_STATE,
                        args={},
                    )
                )
            else:
                outgoing_state = (
                    BLOCKED_STATE
                    if last_end_state in ("S", "D")
                    else RUNNING_STATE
                )
                records.append(
                    trace_model.ContextSwitch(
                        start=timestamp,
                        incoming_tid=tid,
                        outgoing_tid=last_tid,
                        incoming_prio=prio,
                        outgoing_prio=last_prio,
                        outgoing_state=outgoing_state,
                        args={},
                    )
                )

            cpu_states[cpu] = CpuState(
                last_end_ts=ts_ns + raw_dur,
                last_tid=tid,
                last_prio=prio,
                last_end_state=end_state_str,
            )

        return self._SchedulingRecords(scheduling_records)

    def _restore_slice_hierarchy(
        self,
        slice_parent_map: dict[int, int],
        slice_id_to_event: dict[int, trace_model.Event],
    ) -> None:
        """Restores parent-child relationships between DurationEvents in-place.

        Perfetto's `slice` table represents slice nesting via `parent_id` foreign
        keys. However, `trace_model.DurationEvent` objects rely on explicit object-level
        tree relationships (`child.parent` references and `parent_event.child_durations`
        lists) so that metrics processors and model utilities can traverse nested duration
        stacks and sub-events.

        This method iterates over the slice parent mappings to construct those
        bidirectional parent-child links between instanced DurationEvent objects.
        """
        for slice_id, parent_id in slice_parent_map.items():
            if slice_id in slice_id_to_event and parent_id in slice_id_to_event:
                child = slice_id_to_event[slice_id]
                parent_event = slice_id_to_event[parent_id]
                if isinstance(child, trace_model.DurationEvent) and isinstance(
                    parent_event, trace_model.DurationEvent
                ):
                    child.parent = parent_event
                    parent_event.child_durations.append(child)

    def get_events(self) -> _Events:
        slices = self.get_slices()
        counters = self.get_counter_events()
        flows = self.get_flow_events(
            slices.id_to_event,
            slices.parent_map,
        )

        result_events: list[trace_model.Event] = (
            slices.events + counters.events + flows.events
        )

        self._restore_slice_hierarchy(slices.parent_map, slices.id_to_event)

        result_events.sort(
            key=lambda e: (
                e.start,
                0
                if isinstance(e, trace_model.DurationEvent)
                and getattr(e, "duration", None) is not None
                else (
                    1
                    if isinstance(e, trace_model.FlowEvent)
                    else (2 if isinstance(e, trace_model.DurationEvent) else 3)
                ),
            )
        )

        return self._Events(result_events)
