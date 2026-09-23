// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cmp::min;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::iter;

/// A directed graph, whose nodes contain an identifier of type `T`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectedGraph<T: Clone + PartialEq + Hash + Ord + Debug + Display>(
    HashMap<T, DirectedNode<T>>,
);

impl<T: Clone + PartialEq + Hash + Ord + Debug + Display> DirectedGraph<T> {
    /// Created a new empty `DirectedGraph`.
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    /// Remove all edges that don't match the given predicate.
    pub fn retain(&mut self, predicate: impl Fn(&T, &T) -> bool) {
        let mut dangling_nodes = HashSet::new();
        for (k, set) in &mut self.0 {
            set.0.retain(|v| predicate(k, v));
            if set.0.is_empty() {
                dangling_nodes.insert(k.clone());
            }
        }
        // Prune empty nodes that are no longer a target of another node
        for (_, set) in &self.0 {
            for v in &set.0 {
                let _ = dangling_nodes.remove(v);
            }
        }
        self.0.retain(|k, _| !dangling_nodes.contains(k));
    }

    pub fn extend(&mut self, other: impl IntoIterator<Item = (T, T)>) {
        for (a, b) in other.into_iter() {
            self.add_edge(a, b);
        }
    }

    /// Add an edge to the graph, adding nodes if necessary.
    pub fn add_edge(&mut self, source: T, target: T) {
        self.0.entry(source).or_insert_with(DirectedNode::new).add_target(target.clone());
        self.0.entry(target).or_insert_with(DirectedNode::new);
    }

    /// Get targets of all edges from this node.
    pub fn get_targets<'a>(&'a self, id: &T) -> Option<&'a HashSet<T>> {
        self.0.get(id).as_ref().map(|node| &node.0)
    }

    /// Given a dependency graph, find the set of all nodes that have a
    /// dependency on the `start` node (i.e., the reverse dependency closure).
    /// This includes `start` itself.
    pub fn get_closure<'a>(&'a self, start: &T) -> HashSet<&'a T> {
        let Some((start, _)) = self.0.get_key_value(start) else {
            return HashSet::new();
        };
        let mut reverse_deps: HashMap<&T, Vec<&T>> = HashMap::new();
        for (source, targets) in &self.0 {
            for target in &targets.0 {
                reverse_deps.entry(target).or_default().push(source);
            }
        }

        let mut closure = HashSet::new();
        let mut to_visit = VecDeque::new();

        closure.insert(start);
        to_visit.push_back(start);

        while let Some(current_node) = to_visit.pop_front() {
            if let Some(parents) = reverse_deps.get(&current_node) {
                for parent in parents {
                    if closure.insert(*parent) {
                        to_visit.push_back(*parent);
                    }
                }
            }
        }

        closure
    }

    /// Adds an edgeless node to the graph. Used by shutdown to ensure every node gets visited,
    /// even if there are no paths to the node.
    pub fn add_node(&mut self, source: T) {
        self.0.entry(source).or_insert_with(DirectedNode::new);
    }

    /// Returns the nodes of the graph in reverse topological order, or an error if the graph
    /// contains a cycle.
    pub fn topological_sort<'a>(&'a self) -> Result<Vec<&'a T>, Error<'_, T>> {
        TarjanSCC::new(self).run()
    }

    /// Finds the shortest path between the `from` and `to` nodes in this graph, if such a path
    /// exists. Both `from` and `to` are included in the returned path.
    pub fn find_shortest_path<'a>(&'a self, from: &T, to: &T) -> Option<Vec<&'a T>> {
        // Keeps track of edges in the shortest path to each node.
        //
        // The key in this map is a node whose shortest path to it is known. The value
        // is the next-to-last node in the shortest path to the key node.
        //
        // For example, if the shortest path from `a` to `b` is `{a, b, c}`, this
        // map will contain:
        // (c, b)
        // (b, a)
        let mut shortest_path_edges: HashMap<&'a T, &'a T> = HashMap::new();
        let from = self.0.get_key_value(from).map(|e| e.0)?;
        let to = self.0.get_key_value(to).map(|e| e.0)?;

        // Nodes which we have found in the graph but have not yet been visited.
        let mut discovered_nodes = VecDeque::new();
        discovered_nodes.push_back(from);

        loop {
            // Visit the first node in the list.
            let Some(current_node) = discovered_nodes.pop_front() else {
                // If there are no more nodes to visit, then a shortest path must not exist.
                return None;
            };
            match self.get_targets(current_node) {
                None => continue,
                Some(targets) if targets.is_empty() => continue,
                Some(targets) => {
                    for target in targets {
                        // If we haven't yet visited this node, add it to our set of edges and add
                        // it to the set of nodes we should visit.
                        if !shortest_path_edges.contains_key(target) {
                            shortest_path_edges.insert(target, current_node);
                            discovered_nodes.push_back(target);
                        }
                        // If this node is the node we're searching for a path to, then compute the
                        // path based on the hashmap we've built and return it.
                        if target == to {
                            let mut result = vec![target];
                            let mut path_node: &T = target;
                            loop {
                                path_node = shortest_path_edges.get(&path_node).unwrap();
                                result.push(path_node);
                                if path_node == from {
                                    break;
                                }
                            }
                            result.reverse();
                            return Some(result);
                        }
                    }
                }
            }
        }
    }
}

impl<T: Clone + PartialEq + Hash + Ord + Debug + Display, const N: usize> From<[(T, T); N]>
    for DirectedGraph<T>
{
    fn from(items: [(T, T); N]) -> Self {
        let mut this = Self::new();
        for (a, b) in IntoIterator::into_iter(items) {
            this.add_edge(a, b);
        }
        this
    }
}

impl<T: Clone + PartialEq + Hash + Ord + Debug + Display> From<Box<[(T, T)]>> for DirectedGraph<T> {
    fn from(items: Box<[(T, T)]>) -> Self {
        let mut this = Self::new();
        for (a, b) in IntoIterator::into_iter(items) {
            this.add_edge(a, b);
        }
        this
    }
}

impl<T: Clone + PartialEq + Hash + Ord + Debug + Display + 'static> IntoIterator
    for DirectedGraph<T>
{
    type Item = (T, T);
    type IntoIter = Box<dyn Iterator<Item = (T, T)>>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(
            self.0
                .into_iter()
                .map(|(k, set)| iter::zip(iter::repeat(k), set.0.into_iter()))
                .flatten(),
        )
    }
}

impl<T: Clone + PartialEq + Hash + Ord + Debug + Display> Default for DirectedGraph<T> {
    fn default() -> Self {
        Self(HashMap::new())
    }
}

/// A graph node. Contents contain the nodes mapped by edges from this node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectedNode<T: Clone + PartialEq + Hash + Ord + Debug + Display>(HashSet<T>);

impl<T: Clone + PartialEq + Hash + Ord + Debug + Display> DirectedNode<T> {
    /// Create an empty node.
    pub fn new() -> Self {
        Self(HashSet::new())
    }

    /// Add edge from this node to `target`.
    pub fn add_target(&mut self, target: T) {
        self.0.insert(target);
    }
}

/// Errors produced by `DirectedGraph`.
#[derive(Debug)]
pub enum Error<'a, T: Clone + PartialEq + Hash + Ord + Debug + Display> {
    CyclesDetected(HashSet<Vec<&'a T>>),
}

impl<'a, T: Clone + PartialEq + Hash + Ord + Debug + Display> Error<'a, T> {
    pub fn format_cycle(&self) -> String {
        match &self {
            Error::CyclesDetected(cycles) => {
                // Copy the cycles into a vector and sort them so our output is stable
                let mut cycles: Vec<_> = cycles.iter().cloned().collect();
                cycles.sort_unstable();

                let mut output = "{".to_string();
                for cycle in cycles.iter() {
                    output.push_str("{");
                    for item in cycle.iter() {
                        output.push_str(&format!("{} -> ", item));
                    }
                    if !cycle.is_empty() {
                        output.truncate(output.len() - 4);
                    }
                    output.push_str("}, ");
                }
                if !cycles.is_empty() {
                    output.truncate(output.len() - 2);
                }
                output.push_str("}");
                output
            }
        }
    }
}

/// Runs the tarjan strongly connected components algorithm on a graph to produce either a reverse
/// topological sort of the nodes in the graph, or a set of the cycles present in the graph.
///
/// Description of algorithm:
/// https://en.wikipedia.org/wiki/Tarjan%27s_strongly_connected_components_algorithm
struct TarjanSCC<'a, T: Clone + PartialEq + Hash + Ord + Debug + Display> {
    // Each node is assigned an index in the order we find them. This tracks the next index to use.
    index: u64,
    // The mappings between nodes and indices
    indices: HashMap<&'a T, u64>,
    // The lowest index (numerically) that's accessible from each node
    low_links: HashMap<&'a T, u64>,
    // The set of nodes we're currently in the process of considering
    stack: Vec<&'a T>,
    // A set containing the nodes in the stack, so we can more efficiently check if an element is
    // in the stack
    on_stack: HashSet<&'a T>,
    // Detected cycles
    cycles: HashSet<Vec<&'a T>>,
    // Nodes sorted by reverse topological order
    node_order: Vec<&'a T>,
    // The graph this run will be operating on
    graph: &'a DirectedGraph<T>,
}

impl<'a, T: Clone + Hash + Ord + Debug + Display> TarjanSCC<'a, T> {
    fn new(graph: &'a DirectedGraph<T>) -> Self {
        TarjanSCC {
            index: 0,
            indices: HashMap::new(),
            low_links: HashMap::new(),
            stack: Vec::new(),
            on_stack: HashSet::new(),
            cycles: HashSet::new(),
            node_order: Vec::new(),
            graph,
        }
    }

    /// Runs the tarjan scc algorithm. Must only be called once, as it will panic on subsequent
    /// calls.
    fn run(&mut self) -> Result<Vec<&'a T>, Error<'a, T>> {
        // Sort the nodes we visit, to make the output deterministic instead of being based on
        // whichever node we find first.
        let mut nodes: Vec<_> = self.graph.0.keys().collect();
        nodes.sort_unstable();
        for node in &nodes {
            // Iterate over each node, visiting each one we haven't already visited. We determine
            // if a node has been visited by if an index has been assigned to it yet.
            if !self.indices.contains_key(node) {
                self.visit(*node);
            }
        }

        if self.cycles.is_empty() {
            Ok(std::mem::take(&mut self.node_order))
        } else {
            Err(Error::CyclesDetected(std::mem::take(&mut self.cycles)))
        }
    }

    fn visit(&mut self, current_node: &'a T) {
        // assign a new index for this node, and push it on to the stack
        self.indices.insert(current_node, self.index);
        self.low_links.insert(current_node, self.index);
        self.index += 1;
        self.stack.push(current_node);
        self.on_stack.insert(current_node);

        let mut targets: Vec<_> = self.graph.0[current_node].0.iter().collect();
        targets.sort_unstable();

        for target in targets {
            if !self.indices.contains_key(target) {
                // Target has not yet been visited; recurse on it
                self.visit(target);
                // Set our lowlink to the min of our lowlink and the target's new lowlink
                let current_node_low_link = *self.low_links.get(&current_node).unwrap();
                let target_low_link = *self.low_links.get(&target).unwrap();
                self.low_links.insert(current_node, min(current_node_low_link, target_low_link));
            } else if self.on_stack.contains(target) {
                let current_node_low_link = *self.low_links.get(&current_node).unwrap();
                let target_index = *self.indices.get(&target).unwrap();
                self.low_links.insert(current_node, min(current_node_low_link, target_index));
            }
        }

        // If current_node is a root node, pop the stack and generate an SCC
        if self.low_links.get(&current_node) == self.indices.get(&current_node) {
            let mut strongly_connected_nodes = HashSet::new();
            let mut stack_node;
            loop {
                stack_node = self.stack.pop().unwrap();
                self.on_stack.remove(&stack_node);
                strongly_connected_nodes.insert(stack_node);
                if stack_node == current_node {
                    break;
                }
            }
            self.insert_cycles_from_scc(
                &strongly_connected_nodes,
                stack_node,
                HashSet::new(),
                vec![],
            );
        }
        self.node_order.push(current_node);
    }

    /// Given a set of strongly connected components, computes the cycles present in the set and
    /// adds those cycles to self.cycles.
    fn insert_cycles_from_scc(
        &mut self,
        scc_nodes: &HashSet<&'a T>,
        current_node: &'a T,
        mut visited_nodes: HashSet<&'a T>,
        mut path: Vec<&'a T>,
    ) {
        if visited_nodes.contains(&current_node) {
            // We've already visited this node, we've got a cycle. Grab all the elements in the
            // path starting at the first time we visited this node.
            let (current_node_path_index, _) =
                path.iter().enumerate().find(|(_, val)| val == &&current_node).unwrap();
            let mut cycle = path[current_node_path_index..].to_vec();

            // Rotate the cycle such that the lowest value comes first, so that the cycles we
            // report are consistent.
            Self::rotate_cycle(&mut cycle);
            // Push a copy of the first node on to the end, so it's clear that this path ends where
            // it starts
            cycle.push(*cycle.first().unwrap());
            self.cycles.insert(cycle);
            return;
        }

        visited_nodes.insert(current_node);
        path.push(current_node);

        let targets_in_scc: Vec<_> =
            self.graph.0[&current_node].0.iter().filter(|n| scc_nodes.contains(n)).collect();
        for target in targets_in_scc {
            self.insert_cycles_from_scc(scc_nodes, target, visited_nodes.clone(), path.clone());
        }
    }

    /// Rotates the cycle such that ordering is maintained and the lowest element comes first. This
    /// is so that the reported cycles are consistent, as opposed to varying based on which node we
    /// happened to find first.
    fn rotate_cycle(cycle: &mut Vec<&'a T>) {
        let mut lowest_index = 0;
        let mut lowest_value = cycle.first().unwrap();
        for (index, node) in cycle.iter().enumerate() {
            if node < lowest_value {
                lowest_index = index;
                lowest_value = node;
            }
        }
        cycle.rotate_left(lowest_index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! test_topological_sort {
        (
            $(
                $test_name:ident => {
                    edges = $edges:expr,
                    order = $order:expr,
                },
            )+
        ) => {
            $(
                #[test]
                fn $test_name() {
                    topological_sort_test(&$edges, &$order);
                }
            )+
        }
    }

    macro_rules! test_cycles {
        (
            $(
                $test_name:ident => {
                    edges = $edges:expr,
                    cycles = $cycles:expr,
                },
            )+
        ) => {
            $(
                #[test]
                fn $test_name() {
                    cycles_test(&$edges, &$cycles);
                }
            )+
        }
    }

    macro_rules! test_shortest_path {
        (
            $(
                $test_name:ident => {
                    edges = $edges:expr,
                    from = $from:expr,
                    to = $to:expr,
                    shortest_path = $shortest_path:expr,
                },
            )+
        ) => {
            $(
                #[test]
                fn $test_name() {
                    shortest_path_test($edges, $from, $to, $shortest_path);
                }
            )+
        }
    }

    fn topological_sort_test(edges: &[(&'static str, &'static str)], order: &[&'static str]) {
        let mut graph = DirectedGraph::new();
        edges.iter().for_each(|e| graph.add_edge(e.0, e.1));
        let actual_order = graph.topological_sort().expect("found a cycle");

        let expected_order: Vec<_> = order.iter().collect();
        assert_eq!(expected_order, actual_order);
    }

    fn cycles_test(edges: &[(&'static str, &'static str)], cycles: &[&[&'static str]]) {
        let mut graph = DirectedGraph::new();
        edges.iter().for_each(|e| graph.add_edge(e.0, e.1));
        let Error::CyclesDetected(reported_cycles) = graph
            .topological_sort()
            .expect_err("topological sort succeeded on a dataset with a cycle");

        let expected_cycles: HashSet<Vec<_>> =
            cycles.iter().cloned().map(|c| c.iter().collect()).collect();
        assert_eq!(reported_cycles, expected_cycles);
    }

    fn shortest_path_test(
        edges: &[(&'static str, &'static str)],
        from: &'static str,
        to: &'static str,
        expected_shortest_path: Option<&[&'static str]>,
    ) {
        let mut graph = DirectedGraph::new();
        edges.iter().for_each(|e| graph.add_edge(e.0, e.1));
        let actual_shortest_path = graph.find_shortest_path(&from, &to);
        let expected_shortest_path =
            expected_shortest_path.map(|path| path.iter().collect::<Vec<_>>());
        assert_eq!(actual_shortest_path, expected_shortest_path);
    }

    #[test]
    fn operations() {
        fn assert_elements(
            graph: &DirectedGraph<&'static str>,
            expected: &[(&'static str, &'static str)],
        ) {
            let mut elements: Vec<_> = graph.clone().into_iter().collect();
            elements.sort_unstable();
            assert_eq!(&elements, expected);
        }

        let mut graph = DirectedGraph::new();
        graph.add_edge("a", "b");
        assert_elements(&graph, &[("a", "b")]);

        graph.extend(vec![("c", "b"), ("a", "e")]);
        assert_elements(&graph, &[("a", "b"), ("a", "e"), ("c", "b")]);

        graph.retain(|k, v| *k == "c" || *v != "e");
        assert_elements(&graph, &[("a", "b"), ("c", "b")]);

        graph.retain(|k, _| *k != "a");
        assert_elements(&graph, &[("c", "b")]);
        // This confirms that the now empty target set for "a" was removed by the last call to
        // retain().
        let mut expected = DirectedGraph::new();
        expected.add_edge("c", "b");
        assert_eq!(graph, expected);
    }

    // Tests with no cycles

    test_topological_sort! {
        test_empty => {
            edges = [],
            order = [],
        },
        test_fan_out => {
            edges = [
                ("a", "b"),
                ("b", "c"),
                ("b", "d"),
                ("d", "e"),
            ],
            order = ["c", "e", "d", "b", "a"],
        },
        test_fan_in => {
            edges = [
                ("a", "b"),
                ("b", "d"),
                ("c", "d"),
                ("d", "e"),
            ],
            order = ["e", "d", "b", "a", "c"],
        },
        test_forest => {
            edges = [
                ("a", "b"),
                ("b", "c"),
                ("d", "e"),
            ],
            order = ["c", "b", "a", "e", "d"],
        },
        test_diamond => {
            edges = [
                ("a", "b"),
                ("a", "c"),
                ("b", "d"),
                ("c", "d"),
            ],
            order = ["d", "b", "c", "a"],
        },
        test_lattice => {
            edges = [
                ("a", "b"),
                ("a", "c"),
                ("b", "d"),
                ("b", "e"),
                ("c", "d"),
                ("e", "f"),
                ("d", "f"),
            ],
            order = ["f", "d", "e", "b", "c", "a"],
        },
        test_deduped_edge => {
            edges = [
                ("a", "b"),
                ("a", "b"),
                ("b", "c"),
            ],
            order = ["c", "b", "a"],
        },
    }

    test_cycles! {
        // Tests where only 1 SCC contains cycles

        test_cycle_self_referential => {
            edges = [
                ("a", "a"),
            ],
            cycles = [
                &["a", "a"],
            ],
        },
        test_cycle_two_nodes => {
            edges = [
                ("a", "b"),
                ("b", "a"),
            ],
            cycles = [
                &["a", "b", "a"],
            ],
        },
        test_cycle_two_nodes_with_path_in => {
            edges = [
                ("a", "b"),
                ("b", "c"),
                ("c", "d"),
                ("d", "c"),
            ],
            cycles = [
                &["c", "d", "c"],
            ],
        },
        test_cycle_two_nodes_with_path_out => {
            edges = [
                ("a", "b"),
                ("b", "a"),
                ("b", "c"),
                ("c", "d"),
            ],
            cycles = [
                &["a", "b", "a"],
            ],
        },
        test_cycle_three_nodes => {
            edges = [
                ("a", "b"),
                ("b", "c"),
                ("c", "a"),
            ],
            cycles = [
                &["a", "b", "c", "a"],
            ],
        },
        test_cycle_three_nodes_with_inner_cycle => {
            edges = [
                ("a", "b"),
                ("b", "c"),
                ("c", "b"),
                ("c", "a"),
            ],
            cycles = [
                &["a", "b", "c", "a"],
                &["b", "c", "b"],
            ],
        },
        test_cycle_three_nodes_doubly_linked => {
            edges = [
                ("a", "b"),
                ("b", "a"),
                ("b", "c"),
                ("c", "b"),
                ("c", "a"),
                ("a", "c"),
            ],
            cycles = [
                &["a", "b", "a"],
                &["b", "c", "b"],
                &["a", "c", "a"],
                &["a", "b", "c", "a"],
                &["a", "c", "b", "a"],
            ],
        },
        test_cycle_with_inner_cycle => {
            edges = [
                ("a", "b"),
                ("b", "c"),
                ("c", "a"),

                ("b", "d"),
                ("d", "e"),
                ("e", "c"),
            ],
            cycles = [
                &["a", "b", "c", "a"],
                &["a", "b", "d", "e", "c", "a"],
            ],
        },
        test_two_join_cycles => {
            edges = [
                ("a", "b"),
                ("b", "c"),
                ("c", "a"),
                ("b", "d"),
                ("d", "a"),
            ],
            cycles = [
                &["a", "b", "c", "a"],
                &["a", "b", "d", "a"],
            ],
        },
        test_cycle_four_nodes_doubly_linked => {
            edges = [
                ("a", "b"),
                ("b", "a"),
                ("b", "c"),
                ("c", "b"),
                ("c", "d"),
                ("d", "c"),
                ("d", "a"),
                ("a", "d"),
            ],
            cycles = [
                &["a", "b", "c", "d", "a"],
                &["a", "b", "a"],
                &["a", "d", "c", "b", "a"],
                &["a", "d", "a"],
                &["b", "c", "b"],
                &["c", "d", "c"],
            ],
        },

        // Tests with multiple SCCs that contain cycles

        test_cycle_self_referential_islands => {
            edges = [
                ("a", "a"),
                ("b", "b"),
                ("c", "c"),
                ("d", "e"),
            ],
            cycles = [
                &["a", "a"],
                &["b", "b"],
                &["c", "c"],
            ],
        },
        test_cycle_two_sets_of_two_nodes => {
            edges = [
                ("a", "b"),
                ("b", "a"),
                ("c", "d"),
                ("d", "c"),
            ],
            cycles = [
                &["a", "b", "a"],
                &["c", "d", "c"],
            ],
        },
        test_cycle_two_sets_of_two_nodes_connected => {
            edges = [
                ("a", "b"),
                ("b", "a"),
                ("c", "d"),
                ("d", "c"),
                ("a", "c"),
            ],
            cycles = [
                &["a", "b", "a"],
                &["c", "d", "c"],
            ],
        },
    }

    test_shortest_path! {
        test_empty_graph => {
            edges = &[],
            from = "a",
            to = "b",
            shortest_path = None,
        },
        test_two_nodes => {
            edges = &[
                ("a", "b"),
            ],
            from = "a",
            to = "b",
            shortest_path = Some(&["a", "b"]),
        },
        test_path_to_self => {
            edges = &[
                ("a", "a"),
            ],
            from = "a",
            to = "a",
            shortest_path = Some(&["a", "a"]),
        },
        test_path_to_self_no_edge => {
            edges = &[
                ("a", "b"),
            ],
            from = "a",
            to = "a",
            shortest_path = None,
        },
        test_path_three_nodes => {
            edges = &[
                ("a", "b"),
                ("b", "c"),
            ],
            from = "a",
            to = "c",
            shortest_path = Some(&["a", "b", "c"]),
        },
        test_path_multiple_options => {
            edges = &[
                ("a", "b"),
                ("b", "c"),
                ("a", "c"),
            ],
            from = "a",
            to = "c",
            shortest_path = Some(&["a", "c"]),
        },
        test_path_two_islands => {
            edges = &[
                ("a", "b"),
                ("c", "d"),
            ],
            from = "a",
            to = "d",
            shortest_path = None,
        },
        test_path_with_cycle => {
            edges = &[
                ("a", "b"),
                ("b", "a"),
            ],
            from = "a",
            to = "b",
            shortest_path = Some(&["a", "b"]),
        },
        test_path_with_cycle_2 => {
            edges = &[
                ("a", "b"),
                ("b", "c"),
                ("c", "b"),
            ],
            from = "a",
            to = "b",
            shortest_path = Some(&["a", "b"]),
        },
        test_path_with_cycle_3 => {
            edges = &[
                ("a", "b"),
                ("b", "c"),
                ("c", "b"),
                ("b", "d"),
                ("d", "e"),
            ],
            from = "a",
            to = "e",
            shortest_path = Some(&["a", "b", "d", "e"]),
        },
    }
}
