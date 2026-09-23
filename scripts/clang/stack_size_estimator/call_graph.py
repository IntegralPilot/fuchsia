# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Call graph and stack size parsing module."""

from collections import defaultdict
from typing import Any

import graph

# Type aliases
JsonData = dict[str, Any]
StackSizeMap = dict[str, int]
Address = int
FuncName = str
FunctionInfoByAddr = dict[Address, dict[str, Any]]


def extract_type_id_mapping(
    data: list[JsonData], excluded_funcs: set[str] | None = None
) -> dict[int, list[Address]]:
    """
    Extracts a mapping from TypeID to a list of function addresses.
    """
    if excluded_funcs is None:
        excluded_funcs = set()
    type_id_to_targets: dict[int, list[Address]] = defaultdict(list)
    for file_entry in data:
        for entry in file_entry.get("CallGraph", []):
            func = entry.get("Function", {})
            addr = func.get("Address")
            if addr is None:
                continue
            names = func.get("Names", [])
            # Check exclusion
            if not set(names).isdisjoint(excluded_funcs):
                continue
            type_id = func.get("TypeID")
            if type_id is not None:
                type_id_to_targets[type_id].append(addr)
    return type_id_to_targets


def _register_vertices(
    data: list[JsonData],
    excluded_funcs: set[str],
    stack_sizes: StackSizeMap,
    call_graph: graph.Graph,
    fn_name_to_addr: dict[FuncName, Address],
    function_info_map: FunctionInfoByAddr,
) -> None:
    """Registers function vertices and their stack sizes."""
    for file_entry in data:
        for entry in file_entry.get("CallGraph", []):
            func = entry.get("Function", {})
            addr = func.get("Address")
            names = func.get("Names", [])

            if addr is None:
                continue

            primary_name = names[0] if names else f"<unknown_@{hex(addr)}>"

            if not set(names).isdisjoint(excluded_funcs):
                continue

            fn_name_to_addr.update(dict.fromkeys(names, addr))
            stack_usage = next(
                (stack_sizes[n] for n in names if n in stack_sizes), 0
            )

            call_graph.add_vertex(addr)
            function_info_map[addr] = {
                "stack_usage": stack_usage,
                "label": primary_name,
                "names": names,
            }


def _add_edges(
    data: list[JsonData],
    excluded_funcs: set[str],
    type_id_to_targets: dict[int, list[Address]],
    call_graph: graph.Graph,
    function_info_map: FunctionInfoByAddr,
    adj_list: dict[Address, dict[str, Any]],
) -> None:
    """Resolves direct/indirect calls into graph edges and adjacency list."""
    for file_entry in data:
        for entry in file_entry.get("CallGraph", []):
            func = entry.get("Function", {})
            caller_addr = func.get("Address")
            names = func.get("Names", [])

            if caller_addr is None:
                continue

            if not set(names).isdisjoint(excluded_funcs):
                continue

            if caller_addr not in adj_list:
                adj_list[caller_addr] = {"direct": [], "indirect": []}

            for callee in func.get("DirectCallees", []):
                callee_addr = callee.get("Address")
                if callee_addr is not None:
                    if callee_addr in function_info_map:
                        call_graph.add_edge(caller_addr, callee_addr)
                        adj_list[caller_addr]["direct"].append(callee_addr)

            for indirect_type_id in func.get("IndirectTypeIDs", []):
                targets = []
                for target_addr in type_id_to_targets.get(indirect_type_id, []):
                    if target_addr in function_info_map:
                        call_graph.add_edge(caller_addr, target_addr)
                        targets.append(target_addr)
                if targets:
                    adj_list[caller_addr]["indirect"].append(
                        {"typeId": indirect_type_id, "targets": targets}
                    )


def build_call_graph(
    data: list[JsonData],
    stack_sizes: StackSizeMap,
    config: dict[str, Any] | None = None,
) -> tuple[
    graph.Graph,
    dict[FuncName, Address],
    FunctionInfoByAddr,
    dict[Address, dict[str, Any]],
]:
    """
    Constructs a directed graph from call graph metadata and stack sizes.

    Algorithm overview:
    1. Parses the excluded functions from the configuration so we can ignore
       undesired nodes early.
    2. Passes over the JSON data twice:
       - First pass: Registers all function vertices, resolves primary names,
         and maps their associated stack usages.
       - Second pass: Resolves call edges. Direct calls are mapped directly via
         addresses. Indirect calls are resolved by matching IndirectTypeIDs
         to target addresses.

    Invariants:
    - Nodes (vertices) are strictly unique integer addresses.
    - An edge is added only if both the caller and the callee exist in the
      first pass of vertex registration.
    - Excluded functions are skipped entirely, generating no vertices or edges.
    """
    call_graph = graph.Graph()
    fn_name_to_addr: dict[FuncName, Address] = {}
    function_info_map: FunctionInfoByAddr = {}

    excluded_funcs = set()
    if config:
        exclusions = config.get("exclude_functions", [])
        if exclusions:
            first_item = exclusions[0]
            if isinstance(first_item, str):
                excluded_funcs = set(exclusions)
            elif isinstance(first_item, dict):
                excluded_funcs = set(
                    item.get("name") for item in exclusions if "name" in item
                )

    # Map TypeID -> List[Address]
    type_id_to_targets = extract_type_id_mapping(data, excluded_funcs)

    _register_vertices(
        data,
        excluded_funcs,
        stack_sizes,
        call_graph,
        fn_name_to_addr,
        function_info_map,
    )

    adj_list: dict[Address, dict[str, Any]] = {}

    _add_edges(
        data,
        excluded_funcs,
        type_id_to_targets,
        call_graph,
        function_info_map,
        adj_list,
    )

    return call_graph, fn_name_to_addr, function_info_map, adj_list


def parse_stack_sizes(stack_data: list[JsonData]) -> StackSizeMap:
    """
    Parses the stack size JSON data into a simple dictionary.
    """
    sizes: StackSizeMap = {}
    # The format is typically a list of file summaries, each containing
    # "StackSizes" or just a list of stack sizes depending on how it's
    # concatenated. llvm-readelf --stack-sizes --elf-output-style=JSON output:
    # [{ "StackSizes": [ { "Entry": { "Functions": [...], "Size": 10 } } ] }]

    for file_summary in stack_data:
        for entry_item in file_summary.get("StackSizes", []):
            entry = entry_item.get("Entry", {})
            size = entry.get("Size", 0)
            for func_name in entry.get("Functions", []):
                sizes[func_name] = size
    return sizes
