# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Query executors for perf-analyze."""

import json
import os
from typing import Any, Sequence, overload

from plugins import SectionResult
from tp_shell import PerfettoTraceProcessor


class TpShell:
    """Runs raw SQL queries via Perfetto TraceProcessor."""

    @overload
    def query(
        self,
        trace_path: str,
        sql: str,
        batch: None = None,
        cache: bool = True,
    ) -> Sequence[dict[str, Any]]:
        ...

    @overload
    def query(
        self,
        trace_path: str,
        sql: None = None,
        *,
        batch: str,
        cache: bool = True,
    ) -> Sequence[SectionResult]:
        ...

    @overload
    def query(
        self,
        trace_path: str,
        sql: str | None = None,
        batch: str | None = None,
        cache: bool = True,
    ) -> Sequence[dict[str, Any]] | Sequence[SectionResult]:
        ...

    def query(
        self,
        trace_path: str,
        sql: str | None = None,
        batch: str | None = None,
        cache: bool = True,
    ) -> Sequence[dict[str, Any]] | Sequence[SectionResult]:
        """Executes the query specified by args on the given trace.

        Args:
            trace_path: Path to the trace file to ingest.
            sql: Optional raw SQL query string to execute.
            batch: Optional JSON string or @filepath of batch queries to execute.
            cache: If True, caches trace locally when downloading (default: True).

        Returns:
            A list of dictionary results or SectionResults representing the query output.
        """
        if not sql and not batch:
            raise ValueError("Either sql or batch must be provided")

        with PerfettoTraceProcessor(trace_path, cache=cache) as tp:
            if sql:
                return tp.run_query(sql)
            elif batch:
                batch_data = self._load_batch(batch)
                results: list[SectionResult] = []
                for item in batch_data:
                    if not isinstance(item, dict):
                        raise ValueError("Batch items must be JSON objects")
                    q_name = item.get("name")
                    q_sql = item.get("sql")
                    if not q_name or not q_sql:
                        raise ValueError(
                            "Batch items must contain 'name' and 'sql'"
                        )
                    try:
                        q_results = tp.run_query(q_sql)
                        results.append(
                            SectionResult(name=q_name, results=q_results)
                        )
                    except Exception as e:
                        results.append(SectionResult(name=q_name, error=str(e)))
                return results
            return []

    def _load_batch(self, batch_arg: str) -> Sequence[dict[str, Any]]:
        """Loads batch queries from JSON or file path starting with '@'."""
        if batch_arg.startswith("@"):
            file_path = batch_arg[1:]
            if not os.path.exists(file_path):
                raise FileNotFoundError(f"Batch file not found: {file_path}")
            with open(file_path, "r", encoding="utf-8") as f:
                content = f.read()
        else:
            content = batch_arg

        try:
            data = json.loads(content)
            if not isinstance(data, list):
                raise ValueError("Batch content must be a JSON array")
            return data
        except json.JSONDecodeError as e:
            raise ValueError(f"Invalid JSON in batch: {e}")
