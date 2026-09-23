# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import importlib.resources
import os
import time
import unittest
import unittest.mock

from tp_shell import FuchsiaPlatformDelegate, PerfettoTraceProcessor


class TpShellTest(unittest.TestCase):
    def test_single_query_and_cleanup(self) -> None:
        """Tests that a single query works and the background process is cleaned up."""
        source = importlib.resources.files("test_data").joinpath(
            "perfetto_golden.fxt"
        )
        with importlib.resources.as_file(source) as trace_path:
            with PerfettoTraceProcessor(str(trace_path)) as tp:
                # Run a query.
                results = tp.run_query("SELECT count(*) as cnt FROM slice")
                self.assertEqual(len(results), 1)
                self.assertGreater(results[0]["cnt"], 0)

                tables = tp.get_tables()
                self.assertIn("slice", tables)

                # Check that process is alive inside the context.
                self.assertTrue(tp._finalizer.alive)

            # Check that process is cleaned up after exiting context.
            self.assertFalse(tp._finalizer.alive)
            with self.assertRaises(RuntimeError):
                tp.run_query("SELECT count(*) as cnt FROM slice")

    def test_multiple_queries_are_fast(self) -> None:
        """Tests that running multiple queries is fast, proving a persistent connection."""
        source = importlib.resources.files("test_data").joinpath(
            "perfetto_golden.fxt"
        )
        with importlib.resources.as_file(source) as trace_path:
            with PerfettoTraceProcessor(str(trace_path)) as tp:
                # Warmup query
                tp.run_query("SELECT count(*) as cnt FROM slice")

                # Measure 3 queries
                start_time = time.time()
                for _ in range(3):
                    tp.run_query("SELECT count(*) as cnt FROM slice")
                elapsed = time.time() - start_time

                # Spawning a new trace_processor_shell subprocess and loading the
                # perfetto_golden.fxt and performning one query would take under 500ms.
                # In this test we are doing 4 queries, and the last 3 should be more or less "free"
                # since they do not re-parse and load the trace file.
                self.assertLess(
                    elapsed,
                    0.5,
                    f"Expected queries to be fast, took {elapsed:.3f}s",
                )

    def test_duration_events_over_50ms(self) -> None:
        """Tests extracting duration events > 50ms."""
        source = importlib.resources.files("test_data").joinpath(
            "perfetto_golden.fxt"
        )
        with importlib.resources.as_file(source) as trace_path:
            with PerfettoTraceProcessor(str(trace_path)) as tp:
                # 50ms = 50,000,000 ns
                results = tp.run_query(
                    "SELECT name, dur FROM slice WHERE dur > 50000000 ORDER BY ts"
                )
                self.assertEqual(len(results), 20)
                event_names = [r["name"] for r in results]
                expected_names = ["example_duration"] * 20
                self.assertEqual(event_names, expected_names)

    @unittest.mock.patch("urllib.request.urlopen")
    def test_http_uri_resolver_timeout(
        self, mock_urlopen: unittest.mock.MagicMock
    ) -> None:
        """Tests that HttpUriResolver.resolve() calls urlopen with a timeout."""
        from tp_shell.tp_utils import HttpUriResolver

        mock_response = unittest.mock.MagicMock()
        mock_response.read.side_effect = [b"data", b""]
        mock_urlopen.return_value = mock_response

        resolver = HttpUriResolver("http://example.com/trace.fxt")
        resolver.resolve()

        mock_urlopen.assert_called_once()
        _, kwargs = mock_urlopen.call_args
        self.assertEqual(kwargs.get("timeout"), 120)

    @unittest.mock.patch("urllib.request.urlopen")
    @unittest.mock.patch("tp_shell.tp_utils.resolve_trace_url")
    def test_http_uri_resolver_permalink(
        self,
        mock_resolve_trace_url: unittest.mock.MagicMock,
        mock_urlopen: unittest.mock.MagicMock,
    ) -> None:
        """Tests that HttpUriResolver resolves perfetto.dev permalinks."""
        from tp_shell.tp_utils import HttpUriResolver

        mock_resolve_trace_url.return_value = (
            "http://example.com/resolved_trace.fxt"
        )

        mock_response = unittest.mock.MagicMock()
        mock_response.read.side_effect = [b"data", b""]
        mock_urlopen.return_value = mock_response

        resolver = HttpUriResolver("https://ui.perfetto.dev/#!/?s=123456789")
        resolver.resolve()

        mock_resolve_trace_url.assert_called_once_with(
            "https://ui.perfetto.dev/#!/?s=123456789"
        )
        mock_urlopen.assert_called_once_with(
            "http://example.com/resolved_trace.fxt",
            context=unittest.mock.ANY,
            timeout=120,
        )

    @unittest.mock.patch("urllib.request.urlopen")
    @unittest.mock.patch("tp_shell.tp_utils.resolve_trace_url")
    def test_trace_caching_and_reuse(
        self,
        mock_resolve_trace_url: unittest.mock.MagicMock,
        mock_urlopen: unittest.mock.MagicMock,
    ) -> None:
        """Tests downloading, caching, and cache-hit reuse of trace URLs."""
        mock_resolve_trace_url.return_value = (
            "http://example.com/resolved_permalink.fxt"
        )
        mock_response = unittest.mock.MagicMock()
        mock_response.__enter__.return_value = mock_response
        mock_response.read.side_effect = [b"test_trace_data", b""]
        mock_urlopen.return_value = mock_response

        url = "https://ui.perfetto.dev/#!/?s=cache_test_permalink_123"
        registry = FuchsiaPlatformDelegate("").default_resolver_registry()
        cached_path = PerfettoTraceProcessor._resolve_and_cache_url(
            url, registry
        )

        try:
            self.assertTrue(os.path.exists(cached_path))
            with open(cached_path, "rb") as f:
                self.assertEqual(f.read(), b"test_trace_data")
            self.assertEqual(mock_urlopen.call_count, 1)

            # Second call should hit the cache and not invoke urlopen again
            cached_path_2 = PerfettoTraceProcessor._resolve_and_cache_url(
                url, registry
            )
            self.assertEqual(cached_path, cached_path_2)
            self.assertEqual(mock_urlopen.call_count, 1)
        finally:
            if os.path.exists(cached_path):
                os.remove(cached_path)


if __name__ == "__main__":
    unittest.main()
