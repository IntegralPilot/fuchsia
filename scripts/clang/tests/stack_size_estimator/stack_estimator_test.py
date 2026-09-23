# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests for stack_estimator."""

import argparse
import io
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

import call_graph
import graph
import stack_estimator
import symbolizer

# This will be set by main() from the command line arguments.
TESTDATA_DIR: pathlib.Path


class TestGraphHelper(unittest.TestCase):
    """Tests for graph.Graph."""

    def test_scc(self) -> None:
        g = graph.Graph()
        # 0 -> 1 -> 2 -> 0 (Cycle)
        # 2 -> 3
        g.add_edge(0, 1)
        g.add_edge(1, 2)
        g.add_edge(2, 0)
        g.add_edge(2, 3)

        sccs = g.get_sccs()
        # Should have {0, 1, 2} and {3}
        self.assertEqual(len(sccs), 2)
        scc_sets = [set(scc) for scc in sccs]
        self.assertIn({0, 1, 2}, scc_sets)
        self.assertIn({3}, scc_sets)

    def test_topological_sort(self) -> None:
        g = graph.Graph()
        # 0 -> 1, 0 -> 2, 1 -> 3, 2 -> 3
        g.add_edge(0, 1)
        g.add_edge(0, 2)
        g.add_edge(1, 3)
        g.add_edge(2, 3)

        order = g.topological_sort()
        self.assertEqual(len(order), 4)
        self.assertEqual(order[0], 0)
        self.assertEqual(order[3], 3)
        # 1 and 2 can be in any order in between
        self.assertSetEqual(set(order[1:3]), {1, 2})


class TestCallGraph(unittest.TestCase):
    """Tests for call_graph.py."""

    def test_parse_stack_sizes(self) -> None:
        with open(TESTDATA_DIR / "test_stack_sizes.json", "r") as f:
            data = json.load(f)
        sizes = call_graph.parse_stack_sizes(data)
        self.assertEqual(sizes["foo"], 16)
        self.assertEqual(sizes["bar"], 32)
        self.assertEqual(sizes["main"], 64)

    def test_extract_type_id_mapping(self) -> None:
        with open(TESTDATA_DIR / "test_indirect.json", "r") as f:
            data = json.load(f)
        mapping = call_graph.extract_type_id_mapping(data)
        self.assertIn(12345, mapping)
        self.assertCountEqual(mapping[12345], [2000, 3000])

    def test_build_call_graph(self) -> None:
        with open(TESTDATA_DIR / "test_call_graph.json", "r") as f:
            cg_data = json.load(f)
        with open(TESTDATA_DIR / "test_stack_sizes.json", "r") as f:
            ss_data = json.load(f)

        stack_sizes = call_graph.parse_stack_sizes(ss_data)
        cg, _, func_info, _ = call_graph.build_call_graph(
            [cg_data], stack_sizes
        )

        self.assertIn(6080, cg.vertices)  # main
        self.assertCountEqual(cg.graph[6080], [6032, 6048, 6064])
        self.assertEqual(func_info[6080]["stack_usage"], 64)


class TestStackEstimator(unittest.TestCase):
    """Tests for stack_estimator.py."""

    def test_calculate_max_stack_usage_dag(self) -> None:
        """Tests calculation of max stack usage for a Directed Acyclic Graph."""
        # A -> B (10), A -> C (20)
        # B -> D (5), C -> D (5)
        # A: 100, B: 50, C: 60, D: 10
        cg = graph.Graph()
        cg.add_edge(1, 2)
        cg.add_edge(1, 3)
        cg.add_edge(2, 4)
        cg.add_edge(3, 4)

        func_info = {
            1: {"stack_usage": 100, "label": "A"},
            2: {"stack_usage": 50, "label": "B"},
            3: {"stack_usage": 60, "label": "C"},
            4: {"stack_usage": 10, "label": "D"},
        }
        sccs = [[1], [2], [3], [4]]

        results = stack_estimator.calculate_max_stack_usage(cg, func_info, sccs)

        # Max path: A -> C -> D = 100 + 60 + 10 = 170
        # SCC IDs might vary, find SCC for 1
        a_scc_id = next(i for i, scc in enumerate(sccs) if 1 in scc)
        self.assertEqual(results[a_scc_id]["max_stack_usage"], 170)

    def test_calculate_max_stack_usage_cycle(self) -> None:
        """Tests calculation of max stack usage when there are cycles (SCCs)."""
        # A -> B -> A (Cycle)
        # B -> C
        # A: 10, B: 20, C: 30
        cg = graph.Graph()
        cg.add_edge(1, 2)
        cg.add_edge(2, 1)
        cg.add_edge(2, 3)

        func_info = {
            1: {"stack_usage": 10, "label": "A"},
            2: {"stack_usage": 20, "label": "B"},
            3: {"stack_usage": 30, "label": "C"},
        }
        # Tarjan should give [[3], [1, 2]] or similar
        sccs = cg.get_sccs()

        results = stack_estimator.calculate_max_stack_usage(cg, func_info, sccs)

        # SCC {A, B} total = 10 + 20 = 30
        # SCC {C} total = 30
        # Max path from {A, B}: 30 + 30 = 60
        ab_scc_id = next(i for i, scc in enumerate(sccs) if 1 in scc)
        self.assertEqual(results[ab_scc_id]["max_stack_usage"], 60)

    def test_indirect_calls(self) -> None:
        with open(TESTDATA_DIR / "test_indirect.json", "r") as f:
            data = json.load(f)

        stack_sizes = call_graph.parse_stack_sizes(data)
        cg, name_to_addr, func_info, _ = call_graph.build_call_graph(
            data, stack_sizes
        )

        sccs = cg.get_sccs()
        results = stack_estimator.calculate_max_stack_usage(cg, func_info, sccs)

        # caller (10) calls target1 (20) and target2 (30) indirectly
        # Max stack should be 10 + max(20, 30) = 40
        caller_addr = name_to_addr["caller"]
        caller_scc_id = next(
            i for i, scc in enumerate(sccs) if caller_addr in scc
        )
        self.assertEqual(results[caller_scc_id]["max_stack_usage"], 40)

    def test_end_to_end_combined(self) -> None:
        with open(TESTDATA_DIR / "test_combined.json", "r") as f:
            data = json.load(f)

        stack_sizes = call_graph.parse_stack_sizes(data)
        cg, name_to_addr, func_info, _ = call_graph.build_call_graph(
            data, stack_sizes
        )

        self.assertIn("main", name_to_addr)
        entry_addr = name_to_addr["main"]

        sccs = cg.get_sccs()
        results = stack_estimator.calculate_max_stack_usage(cg, func_info, sccs)

        main_scc_id = next(i for i, scc in enumerate(sccs) if entry_addr in scc)
        max_stack = results[main_scc_id]["max_stack_usage"]

        # main (64) calls foo (16), bar (32), baz (8)
        # Total = 64 + 32 = 96
        self.assertEqual(max_stack, 96)


class TestSymbolizer(unittest.TestCase):
    """Tests for symbolizer.Symbolizer."""

    @patch.object(subprocess, "Popen")
    def test_symbolizer(self, mock_popen: MagicMock) -> None:
        # Mock the process
        mock_process = MagicMock()
        mock_popen.return_value = mock_process

        # Mock stdout.readline to return a source location
        mock_process.stdout.readline.side_effect = ["main.c:10\n", "foo.c:20\n"]

        # Mock path exists for bin and obj
        with patch.object(os.path, "exists", return_value=True):
            sym = symbolizer.Symbolizer("llvm-symbolizer", "test.elf")
            results = sym.symbolize([0x1000, 0x2000])

            self.assertEqual(results[0x1000], "main.c:10")
            self.assertEqual(results[0x2000], "foo.c:20")

            sym.close()
            mock_process.terminate.assert_called()


class TestStackEstimatorEndToEnd(unittest.TestCase):
    """End-to-End tests simulating CLI usage."""

    def test_multiple_entry_functions(self) -> None:
        """Tests that the script handles multiple entry functions."""
        with tempfile.TemporaryDirectory() as tmp_dir:
            config_path = Path(tmp_dir) / "test_config.json"
            # Create a temporary config file referencing the existing test data
            config_data = {"entry_functions": ["main", "foo"]}
            with open(config_path, "w") as f:
                json.dump(config_data, f)

            callgraph_path = TESTDATA_DIR / "test_combined.json"

            # Capture stdout from a mock run
            # Using patch to redirect stdout and simulate sys.argv
            with patch.object(sys, "stdout", new=io.StringIO()) as fake_stdout:
                with patch.object(
                    sys,
                    "argv",
                    [
                        "stack_estimator.py",
                        str(config_path),
                        str(callgraph_path),
                    ],
                ):
                    stack_estimator.main()

            output_str = fake_stdout.getvalue()
            output_json = json.loads(output_str)

            entry_funcs = output_json["entry_functions"]
            self.assertEqual(len(entry_funcs), 2)

            main_result = next(
                r for r in entry_funcs if r["entry_function"] == "main"
            )
            self.assertEqual(main_result["max_stack_usage"], 96)

            foo_result = next(
                r for r in entry_funcs if r["entry_function"] == "foo"
            )
            self.assertEqual(foo_result["max_stack_usage"], 16)


def main() -> None:
    # unittest.main() can't parse arguments which are not its own,
    # so we need to parse our arguments first.
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--testdata-dir",
        required=True,
        type=pathlib.Path,
        help="Path to testdata",
    )
    # unittest.main() will see the leftover args and complain.
    # We use parse_known_args to separate them out.
    args, remaining_argv = parser.parse_known_args()

    # Make TESTDATA_DIR available to the tests.
    global TESTDATA_DIR
    TESTDATA_DIR = args.testdata_dir

    # Now, run unittest with its own arguments, plus sys.argv[0]
    unittest.main(argv=[sys.argv[0]] + remaining_argv)


if __name__ == "__main__":
    main()
