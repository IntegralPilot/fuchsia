// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#include "lib/system-topology.h"

#include <stddef.h>

namespace system_topology {

// Node and Graph mirror the #[repr(C)] definitions in src/mod.rs, where ktl::span<T> stands in
// for FfiSlice<T>.
struct RawSpanLayout {
  const int* data;
  size_t size;
};
static_assert(__is_layout_compatible(ktl::span<const int>, RawSpanLayout),
              "ktl::span layout must be (pointer, size_t) to match Rust FfiSlice");
static_assert(sizeof(ktl::span<Node*>) == 16);
static_assert(alignof(ktl::span<Node*>) == 8);
static_assert(sizeof(Node) == 80);
static_assert(alignof(Node) == 8);
static_assert(offsetof(Node, entity) == 0);
static_assert(offsetof(Node, parent) == 56);
static_assert(offsetof(Node, children) == 64);
static_assert(sizeof(Graph) == 64);
static_assert(alignof(Graph) == 8);

zx_status_t Graph::Initialize(Graph* graph, const zbi_topology_node_t* flat_nodes, size_t count) {
  // Graph's fields are private, so their offsets are checked from a member.
  static_assert(offsetof(Graph, nodes_) == 0);
  static_assert(offsetof(Graph, processors_) == 16);
  static_assert(offsetof(Graph, logical_processor_count_) == 32);
  static_assert(offsetof(Graph, processors_by_logical_id_) == 40);
  static_assert(offsetof(Graph, backing_) == 56);
  return rust_system_topology_graph_initialize(graph, flat_nodes, count);
}

zx_status_t Graph::InitializeSystemTopology(const zbi_topology_node_t* nodes, size_t count) {
  return rust_system_topology_initialize_system_topology(nodes, count);
}

void Graph::Dump() const { rust_system_topology_graph_dump(this); }

uint8_t GetPerformanceClass(cpu_num_t cpu_id) {
  return rust_system_topology_get_performance_class(cpu_id);
}

}  // namespace system_topology
