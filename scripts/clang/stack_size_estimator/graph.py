# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Graph algorithms and utilities."""
import logging
from typing import Any


class Graph:
    """A simple directed graph implementation."""

    def __init__(self) -> None:
        self.vertices: set[Any] = set()
        self.graph: dict[Any, list[Any]] = {}

    def add_vertex(self, u: Any) -> None:
        """Adds a vertex to the graph."""
        self.vertices.add(u)
        if u not in self.graph:
            self.graph[u] = []

    def add_edge(self, u: Any, v: Any) -> None:
        """Adds a directed edge from u to v."""
        self.add_vertex(u)
        self.add_vertex(v)
        self.graph[u].append(v)

    def get_sccs(self) -> list[list[Any]]:
        """
        Finds Strongly Connected Components using Tarjan's algorithm.
        """
        index_counter = 0
        stack = []
        lowlink = {}
        index = {}
        result = []

        def connect(node: Any) -> None:
            nonlocal index_counter
            index[node] = index_counter
            lowlink[node] = index_counter
            index_counter += 1
            stack.append(node)

            try:
                successors = self.graph.get(node, [])
                for successor in successors:
                    if successor not in index:
                        connect(successor)
                        lowlink[node] = min(lowlink[node], lowlink[successor])
                    elif successor in stack:
                        lowlink[node] = min(lowlink[node], index[successor])
            except RecursionError:
                # A RecursionError occurs when the DFS encounters a deeply
                # nested, linear chain of function calls that exceeds Python's
                # recursion limit (e.g. 1000 nodes deep in a straight line).
                logging.warning(
                    "Recursion error encountered on node %s. "
                    "Graph is too deep.",
                    node,
                )
                # Fallback or handle deep recursion if needed

            if lowlink[node] == index[node]:
                # SCC found. The current node is the root of the SCC.
                # Nodes belonging to this SCC are currently on the stack.
                # The stack is guaranteed to not be empty because 'node'
                # itself was added at the start of connect(node) and
                # hasn't been popped yet.
                connected_component = []
                while True:
                    successor = stack.pop()
                    connected_component.append(successor)
                    if successor == node:
                        break
                result.append(connected_component)

        for node in self.vertices:
            if node not in index:
                connect(node)

        return result

    def topological_sort(self) -> list[Any]:
        """
        Returns vertices in topological order (u before v if u->v).
        Note: This is only valid for DAGs. For cyclic graphs, this returns
        *some* order compatible with the condensation DAG if used on SCCs,
        but here it's run directly on the SCC DAG.
        So this function just needs to support DAGs.
        """
        visited = set()
        stack = []

        def visit(node: Any) -> None:
            visited.add(node)
            for neighbor in self.graph.get(node, []):
                if neighbor not in visited:
                    visit(neighbor)
            stack.append(node)

        for node in self.vertices:
            if node not in visited:
                visit(node)

        # Stack has reverse topological order (children first).
        # We want u before v (parents first).
        return stack[::-1]
