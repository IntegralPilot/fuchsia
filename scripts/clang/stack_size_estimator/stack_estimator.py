# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Estimates maximum stack usage by combining call graph and stack sizes."""

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any

import call_graph
import symbolizer
from graph import Graph

# Type aliases
SccResults = dict[int, dict[str, Any]]
FunctionInfoByAddr = dict[int, dict[str, Any]]


def _build_condensation_graph(
    cg: Graph,
    sccs: list[list[int]],
) -> tuple[Graph, dict[int, int]]:
    node_to_scc_id = {node: i for i, scc in enumerate(sccs) for node in scc}
    scc_dag = Graph()
    for u in cg.vertices:
        scc_id_u = node_to_scc_id.get(u)
        if scc_id_u is None:
            continue
        scc_dag.add_vertex(scc_id_u)

        if u in cg.graph:
            for v in cg.graph[u]:
                scc_id_v = node_to_scc_id.get(v)
                if scc_id_v is not None and scc_id_u != scc_id_v:
                    scc_dag.add_edge(scc_id_u, scc_id_v)
    return scc_dag, node_to_scc_id


def _compute_local_scc_stack_usage(
    function_info_map: FunctionInfoByAddr, sccs: list[list[int]]
) -> dict[int, int]:
    return {
        i: sum(
            function_info_map.get(func_addr, {}).get("stack_usage", 0)
            for func_addr in scc
        )
        for i, scc in enumerate(sccs)
    }


def _calculate_max_paths(
    scc_dag: Graph, scc_stack_usage: dict[int, int]
) -> tuple[dict[int, int], dict[int, int | None]]:
    max_stack_per_scc: dict[int, int] = {}
    scc_next_hop: dict[int, int | None] = {}

    sorted_sccs = scc_dag.topological_sort()
    for scc_id in reversed(sorted_sccs):
        successors = scc_dag.graph.get(scc_id, [])
        max_downstream_usage = 0
        best_next_scc = None

        if successors:
            best_next_scc = max(
                successors, key=lambda succ: max_stack_per_scc.get(succ, 0)
            )
            max_downstream_usage = max_stack_per_scc.get(best_next_scc, 0)

        max_stack_per_scc[scc_id] = (
            scc_stack_usage.get(scc_id, 0) + max_downstream_usage
        )
        scc_next_hop[scc_id] = best_next_scc

    return max_stack_per_scc, scc_next_hop


def _format_scc_results(
    sccs: list[list[int]],
    function_info_map: FunctionInfoByAddr,
    max_stack_per_scc: dict[int, int],
    scc_next_hop: dict[int, int | None],
) -> SccResults:
    results: SccResults = {}
    for scc_id, scc_nodes in enumerate(sccs):
        functions_in_scc = [
            {
                "name": function_info_map.get(addr, {}).get(
                    "label", f"unknown @ {hex(addr)}"
                ),
                "address": addr,
                "stack_usage": function_info_map.get(addr, {}).get(
                    "stack_usage", 0
                ),
            }
            for addr in scc_nodes
        ]
        results[scc_id] = {
            "max_stack_usage": max_stack_per_scc.get(scc_id, 0),
            "functions": functions_in_scc,
            "next_scc_on_max_path": scc_next_hop.get(scc_id),
        }
    return results


def calculate_max_stack_usage(
    cg: Graph,
    function_info_map: FunctionInfoByAddr,
    sccs: list[list[int]],
) -> SccResults:
    """
    Calculates max stack usage and tracks the path for each Strongly Connected Component (SCC).

    Algorithm:
    1. Builds a Condensation Graph (a DAG) by treating each SCC as a single node.
    2. Computes the conservative local stack usage for each SCC by summing up the
       stack sizes of all functions within it.
    3. Performs a bottom-up traversal (reverse topological sort) of the Condensation
       DAG to calculate the maximum cumulative stack usage:
       `MaxStack(SCC) = LocalStack(SCC) + Max(MaxStack(SuccessorSCCs))`
    """
    scc_dag, _ = _build_condensation_graph(cg, sccs)
    scc_local_usage = _compute_local_scc_stack_usage(function_info_map, sccs)
    max_stack_per_scc, scc_next_hop = _calculate_max_paths(
        scc_dag, scc_local_usage
    )
    return _format_scc_results(
        sccs, function_info_map, max_stack_per_scc, scc_next_hop
    )


def _parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Estimate max stack usage from call graph and stack sizes."
    )
    parser.add_argument(
        "config",
        help="Path to configuration JSON file",
    )
    parser.add_argument(
        "callgraph",
        help="Path to call graph JSON file generated from llvm-readelf",
    )
    return parser.parse_args()


def _load_json_file(path: Path | str) -> Any:
    try:
        with open(path, "r") as f:
            return json.load(f)
    except (OSError, ValueError) as e:
        print(f"Error reading JSON file {path}: {e}", file=sys.stderr)
        sys.exit(1)


def _symbolize_addresses(
    llvm_symbolizer_path: str | None, elf_file: str | None, all_addrs: list[int]
) -> dict[int, str]:
    src_map: dict[int, str] = {}
    if (
        llvm_symbolizer_path
        and os.path.exists(llvm_symbolizer_path)
        and elf_file
        and os.path.exists(elf_file)
    ):
        try:
            with symbolizer.Symbolizer(llvm_symbolizer_path, elf_file) as sym:
                src_map = sym.symbolize(all_addrs)
        except OSError as e:
            print(
                f"Warning: Failed to initialize symbolizer: {e}",
                file=sys.stderr,
            )
    return src_map


def _generate_output_json(
    func_info: FunctionInfoByAddr,
    results: SccResults,
    addr_to_scc_id: dict[int, int],
    adj_list: dict[int, dict[str, Any]],
    src_map: dict[int, str],
    entry_functions: list[str],
    name_to_addr: dict[str, int],
) -> dict[str, Any]:
    all_nodes = {}

    for u, info in func_info.items():
        cumulative_stack = 0
        scc_id = addr_to_scc_id.get(u)
        if scc_id is not None and scc_id in results:
            cumulative_stack = results[scc_id]["max_stack_usage"]

        node_data = {
            "name": info.get("label", f"@{hex(u)}"),
            "stack": info.get("stack_usage", 0),
            "max_cumulative_stack": cumulative_stack,
            "sccId": scc_id,
            "direct": [],
            "indirect": [],
        }

        adj = adj_list.get(u, {"direct": [], "indirect": []})
        for v in adj["direct"]:
            node_data["direct"].append(v)
        for ind_group in adj["indirect"]:
            node_data["indirect"].append(ind_group)

        if u in src_map:
            node_data["source"] = src_map[u]

        all_nodes[u] = node_data

    all_cycles = []
    for scc_id, res in results.items():
        funcs = res["functions"]
        if len(funcs) > 1:
            all_cycles.append(
                {
                    "id": scc_id,
                    "max_stack": res["max_stack_usage"],
                    "nodes": [f["address"] for f in funcs],
                }
            )

    entry_funcs_output: list[dict[str, Any]] = []

    for entry_function in entry_functions:
        if entry_function not in name_to_addr:
            entry_funcs_output.append(
                {
                    "entry_function": entry_function,
                    "error": "Entry function not found",
                }
            )
            continue

        entry_addr = name_to_addr[entry_function]
        entry_scc_id = addr_to_scc_id.get(entry_addr)
        max_stack = 0
        if entry_scc_id is not None and entry_scc_id in results:
            max_stack = results[entry_scc_id]["max_stack_usage"]

        has_cycles = any(cycle["id"] == entry_scc_id for cycle in all_cycles)

        entry_funcs_output.append(
            {
                "entry_function": entry_function,
                "scc_id": entry_scc_id,
                "max_stack_usage": max_stack,
                "has_cycles": has_cycles,
            }
        )

    return {
        "nodes": all_nodes,
        "cycles": all_cycles,
        "entry_functions": entry_funcs_output,
    }


def estimate(
    config_path: Path | str, callgraph_path: Path | str
) -> dict[str, Any]:
    """Estimates max stack usage and returns the result as a dictionary."""
    config = _load_json_file(config_path)
    data = _load_json_file(callgraph_path)

    entry_functions = config.get("entry_functions", ["main"])
    llvm_symbolizer_path = config.get("llvm_symbolizer_path")
    elf_file = config.get("elf_file")

    stack_sizes = call_graph.parse_stack_sizes(data)
    cg, name_to_addr, func_info, adj_list = call_graph.build_call_graph(
        data, stack_sizes, config
    )

    # --- Analysis Logic ---
    addr_to_scc_id = {}

    sccs = cg.get_sccs()
    for i, scc in enumerate(sccs):
        for addr in scc:
            addr_to_scc_id[addr] = i

    results = calculate_max_stack_usage(cg, func_info, sccs)

    src_map = _symbolize_addresses(
        llvm_symbolizer_path, elf_file, list(func_info.keys())
    )

    return _generate_output_json(
        func_info,
        results,
        addr_to_scc_id,
        adj_list,
        src_map,
        entry_functions,
        name_to_addr,
    )


def main() -> None:
    """Main entry point for stack estimator tool."""
    args = _parse_arguments()
    output = estimate(args.config, args.callgraph)
    print(json.dumps(output, indent=2))


if __name__ == "__main__":
    main()
