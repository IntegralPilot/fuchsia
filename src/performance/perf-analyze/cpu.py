# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""CPU utilization, wakeups, DVFS processing rates, and power triage plugin.

This plugin provides a 10,000-foot diagnostic view of system activity during an
idle trace, identifying timer thrashing, excessive wakeups, per-core utilization
and frequency scaling, async executor overhead, binder IPC chatter, and suspend/wake
lease blockers.
"""

from collections.abc import Set
from typing import Sequence

from plugins import (
    AnalyzePlugin,
    PluginArgumentError,
    PluginArgumentParser,
    SectionResult,
)
from tp_shell import PerfettoTraceProcessor


def _perform_analysis_query(
    name: str,
    tp: PerfettoTraceProcessor,
    query: str,
    required_tables: Set[str],
    db_objects: Set[str],
) -> SectionResult:
    if missing := required_tables - db_objects:
        return SectionResult(
            name=name,
            error=f"Required schema tables/views missing: {', '.join(sorted(missing))}",
        )
    try:
        results = tp.run_query(query)
        return SectionResult(
            name=name,
            results=results,
        )
    except Exception as e:
        return SectionResult(
            name=name,
            error=f"Query execution failed: {e}",
        )


def _analyze_restless_sleepers(
    tp: PerfettoTraceProcessor,
    db_objects: Set[str],
    limit: int,
) -> SectionResult:
    """Analyzes thread wakeup counts and context switches (Restless Sleepers)."""
    required_tables = {"thread_state", "thread", "process"}
    query = f"""
    SELECT
      t.name AS thread_name,
      t.tid AS tid,
      p.name AS process_name,
      p.pid AS pid,
      COUNT(*) AS wakeup_count,
      ROUND(AVG(ts.dur) / 1e6, 3) AS avg_duration_ms
    FROM thread_state ts
    JOIN thread t USING(utid)
    LEFT JOIN process p USING(upid)
    WHERE ts.state = 'Running'
    GROUP BY utid
    ORDER BY wakeup_count DESC
    LIMIT {limit};
    """
    return _perform_analysis_query(
        "Restless Sleepers (Wakeup Counts)",
        tp,
        query,
        required_tables,
        db_objects,
    )


def _has_power_counters(
    tp: PerfettoTraceProcessor,
    db_objects: Set[str],
) -> bool:
    """Checks whether the trace database contains kernel:power DVFS counter tracks."""
    if {"counter", "process_counter_track", "process"} - db_objects:
        return False
    try:
        rows = tp.run_query(
            "SELECT 1 FROM process_counter_track pct "
            "JOIN process p USING (upid) "
            "WHERE p.name = 'kernel' AND pct.name LIKE 'Processing Rate:CPU:%' "
            "LIMIT 1;"
        )
        return len(rows) > 0
    except Exception:
        return False


def _analyze_core_utilization_from_counters(
    tp: PerfettoTraceProcessor,
    db_objects: Set[str],
) -> SectionResult:
    """Analyzes per-core CPU utilization percentage weighted by DVFS processing rates.

    Reference: https://fuchsia.dev/fuchsia-src/development/tracing/advanced/recording-cpu-frequency
    Docs: //docs/development/tracing/advanced/recording-cpu-frequency.md

    In Fuchsia traces with the 'kernel:power' category, CPU frequency changes appear
    as 'Processing Rate:CPU:N' counter tracks under the 'kernel' process. The values
    are on a normalized scale (1000 = 100% max frequency).
    """
    required_tables = {
        "trace_bounds",
        "thread_state",
        "thread",
        "process",
        "counter",
        "process_counter_track",
    }
    query = """
    WITH trace_window AS (
      SELECT (end_ts - start_ts) AS total_dur, end_ts FROM trace_bounds
    ),
    core_util AS (
      SELECT
        ts.cpu,
        ROUND(SUM(ts.dur) / 1e6, 2) AS running_ms,
        ROUND((CAST(SUM(ts.dur) AS DOUBLE) / (SELECT total_dur FROM trace_window)) * 100.0, 2) AS running_pct
      FROM thread_state ts
      JOIN thread t USING (utid)
      LEFT JOIN process p USING (upid)
      WHERE ts.state = 'Running'
        AND ts.cpu IS NOT NULL
        AND (p.name IS NULL OR p.name != 'idle')
        AND (t.name IS NULL OR t.name != 'idle')
      GROUP BY ts.cpu
    ),
    kernel_rate_raw AS (
      SELECT
        CAST(SUBSTR(pct.name, 21) AS INT) AS cpu,
        c.value AS rate_val,
        (LEAD(c.ts, 1, (SELECT end_ts FROM trace_window)) OVER (
          PARTITION BY pct.id ORDER BY c.ts
        ) - c.ts) AS step_dur
      FROM counter c
      JOIN process_counter_track pct ON c.track_id = pct.id
      JOIN process p USING (upid)
      WHERE p.name = 'kernel' AND pct.name LIKE 'Processing Rate:CPU:%'
    ),
    core_rate AS (
      SELECT
        cpu,
        ROUND(SUM(rate_val * step_dur) / CAST(SUM(step_dur) AS DOUBLE) / 10.0, 1) AS avg_rate_pct,
        ROUND(MIN(rate_val) / 10.0, 1) AS min_rate_pct,
        ROUND(MAX(rate_val) / 10.0, 1) AS max_rate_pct
      FROM kernel_rate_raw
      WHERE cpu IS NOT NULL AND step_dur > 0
      GROUP BY cpu
    )
    SELECT
      u.cpu,
      u.running_ms,
      u.running_pct,
      r.avg_rate_pct,
      r.min_rate_pct,
      r.max_rate_pct,
      ROUND(u.running_pct * COALESCE(r.avg_rate_pct, 100.0) / 100.0, 2) AS effective_load_pct
    FROM core_util u
    LEFT JOIN core_rate r USING (cpu)
    ORDER BY u.cpu;
    """
    return _perform_analysis_query(
        "Per-Core Utilization & Processing Rate",
        tp,
        query,
        required_tables,
        db_objects,
    )


def _analyze_core_utilization_from_average(
    tp: PerfettoTraceProcessor,
    db_objects: Set[str],
) -> SectionResult:
    """Analyzes per-core CPU utilization percentage over the trace window without DVFS rates."""
    required_tables = {"trace_bounds", "thread_state", "thread", "process"}
    query = """
    WITH trace_window AS (
      SELECT (end_ts - start_ts) AS total_dur FROM trace_bounds
    )
    SELECT
      ts.cpu,
      ROUND(SUM(ts.dur) / 1e6, 2) AS running_ms,
      ROUND((CAST(SUM(ts.dur) AS DOUBLE) / (SELECT total_dur FROM trace_window)) * 100.0, 2) AS running_pct,
      NULL AS avg_rate_pct,
      NULL AS min_rate_pct,
      NULL AS max_rate_pct,
      ROUND((CAST(SUM(ts.dur) AS DOUBLE) / (SELECT total_dur FROM trace_window)) * 100.0, 2) AS effective_load_pct
    FROM thread_state ts
    JOIN thread t USING (utid)
    LEFT JOIN process p USING (upid)
    WHERE ts.state = 'Running'
      AND ts.cpu IS NOT NULL
      AND (p.name IS NULL OR p.name != 'idle')
      AND (t.name IS NULL OR t.name != 'idle')
    GROUP BY ts.cpu
    ORDER BY ts.cpu;
    """
    return _perform_analysis_query(
        "Per-Core Utilization & Processing Rate",
        tp,
        query,
        required_tables,
        db_objects,
    )


def _analyze_core_utilization_and_rate(
    tp: PerfettoTraceProcessor,
    db_objects: Set[str],
) -> SectionResult:
    """Analyzes per-core CPU utilization percentage and DVFS processing rates.

    Reference: https://fuchsia.dev/fuchsia-src/development/tracing/advanced/recording-cpu-frequency
    Docs: //docs/development/tracing/advanced/recording-cpu-frequency.md

    In Fuchsia traces with the 'kernel:power' category, CPU frequency changes appear
    as 'Processing Rate:CPU:N' counter tracks under the 'kernel' process. The values
    are on a normalized scale (1000 = 100% max frequency).
    """
    if _has_power_counters(tp, db_objects):
        return _analyze_core_utilization_from_counters(tp, db_objects)
    return _analyze_core_utilization_from_average(tp, db_objects)


def _analyze_usual_suspects(
    tp: PerfettoTraceProcessor,
    db_objects: Set[str],
    limit: int,
) -> SectionResult:
    """Analyzes top CPU runtime consumers across all threads."""
    required_tables = {"thread_state", "thread", "process"}
    query = f"""
    SELECT
      t.name AS thread_name,
      t.tid AS tid,
      p.name AS process_name,
      p.pid AS pid,
      ROUND(SUM(ts.dur) / 1e6, 2) AS total_cpu_ms
    FROM thread_state ts
    JOIN thread t USING(utid)
    LEFT JOIN process p USING(upid)
    WHERE ts.state = 'Running'
    GROUP BY utid
    ORDER BY total_cpu_ms DESC
    LIMIT {limit};
    """
    return _perform_analysis_query(
        "Top CPU Consumers (Usual Suspects)",
        tp,
        query,
        required_tables,
        db_objects,
    )


def _analyze_executor_overhead(
    tp: PerfettoTraceProcessor,
    db_objects: Set[str],
    limit: int,
) -> SectionResult:
    """Analyzes Fuchsia async executor and futures management overhead."""
    required_tables = {"slice", "thread_track", "thread"}
    query = f"""
    SELECT
      t.name AS thread_name,
      t.tid AS tid,
      COUNT(*) AS slice_count,
      ROUND(SUM(s.dur) / 1e6, 2) AS total_duration_ms
    FROM slice s
    JOIN thread_track tr ON s.track_id = tr.id
    JOIN thread t USING(utid)
    WHERE s.name LIKE '%executor%' OR s.name LIKE '%fuchsia_async%'
    GROUP BY utid, t.name, t.tid
    ORDER BY total_duration_ms DESC
    LIMIT {limit};
    """
    return _perform_analysis_query(
        "Fuchsia Async Executor Overhead",
        tp,
        query,
        required_tables,
        db_objects,
    )


def _analyze_binder_overhead(
    tp: PerfettoTraceProcessor,
    db_objects: Set[str],
    limit: int,
) -> SectionResult:
    """Analyzes Starnix/Android Binder IPC transaction volume and latencies."""
    required_tables = {"slice", "thread_track", "thread", "process"}
    query = f"""
    SELECT
      t.name AS thread_name,
      t.tid AS tid,
      p.name AS process_name,
      p.pid AS pid,
      COUNT(*) AS transaction_count,
      ROUND(COALESCE(SUM(s.dur) / 1e6, 0.0), 2) AS total_duration_ms,
      ROUND(COALESCE(MAX(s.dur) / 1e6, 0.0), 2) AS max_duration_ms
    FROM slice s
    JOIN thread_track tr ON s.track_id = tr.id
    JOIN thread t USING (utid)
    LEFT JOIN process p USING (upid)
    WHERE s.name LIKE 'binder transaction%' OR s.name LIKE 'binder reply%'
    GROUP BY utid, t.name, t.tid, p.name, p.pid
    ORDER BY total_duration_ms DESC
    LIMIT {limit};
    """
    return _perform_analysis_query(
        "Binder IPC Breakdown",
        tp,
        query,
        required_tables,
        db_objects,
    )


def _analyze_suspend_wake_leases(
    tp: PerfettoTraceProcessor,
    db_objects: Set[str],
    limit: int,
) -> SectionResult:
    """Analyzes System Activity Governor (SAG) suspend attempts and wake lease events."""
    required_tables = {"slice", "thread_track", "thread", "process"}
    query = f"""
    SELECT
      s.name AS event_name,
      t.name AS thread_name,
      p.name AS process_name,
      COUNT(*) AS occurrences,
      ROUND(COALESCE(SUM(s.dur) / 1e6, 0.0), 2) AS total_duration_ms,
      ROUND(COALESCE(MAX(s.dur) / 1e6, 0.0), 2) AS max_duration_ms
    FROM slice s
    LEFT JOIN thread_track tr ON s.track_id = tr.id
    LEFT JOIN thread t ON tr.utid = t.utid
    LEFT JOIN process p ON t.upid = p.upid
    WHERE s.name LIKE '%wake_lease%' OR s.name LIKE '%suspend%'
    GROUP BY s.name, t.name, p.name
    ORDER BY occurrences DESC
    LIMIT {limit};
    """
    return _perform_analysis_query(
        "Suspend and Wake Lease Tracking",
        tp,
        query,
        required_tables,
        db_objects,
    )


class CpuPlugin(AnalyzePlugin):
    """Analysis plugin for CPU utilization, wakeups, DVFS rates, and idle power diagnostics."""

    name: str = "cpu"
    description: str = (
        "Analyze CPU utilization, wakeups, processing rates, executor overhead, "
        "binder IPC, and suspend/wake leases (optimized for idle/power triage)"
    )

    def analyze(
        self,
        remaining_args: Sequence[str],
        trace_path: str,
        cache: bool = True,
    ) -> Sequence[SectionResult]:
        parser = PluginArgumentParser(
            prog=f"perf-analyze analyze --plugin {self.name}"
        )
        parser.add_argument(
            "--limit",
            type=int,
            default=15,
            help="Maximum number of rows to return for ranked queries (default: 15)",
        )
        args = parser.parse_args(remaining_args)

        if args.limit <= 0:
            raise PluginArgumentError("Limit must be a positive integer.")

        with PerfettoTraceProcessor(trace_path, cache=cache) as tp:
            db_objects = tp.get_tables()
            return [
                _analyze_restless_sleepers(tp, db_objects, args.limit),
                _analyze_core_utilization_and_rate(tp, db_objects),
                _analyze_usual_suspects(tp, db_objects, args.limit),
                _analyze_executor_overhead(tp, db_objects, args.limit),
                _analyze_binder_overhead(tp, db_objects, args.limit),
                _analyze_suspend_wake_leases(tp, db_objects, args.limit),
            ]
