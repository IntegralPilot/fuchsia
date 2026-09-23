// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zbi-format/cpu.h>
#include <lib/zbi-format/internal/deprecated-cpu.h>
#include <lib/zbitl/items/cpu-topology.h>

#include <algorithm>

namespace zbitl {

fit::result<std::string_view, CpuTopologyTable> CpuTopologyTable::FromPayload(
    uint32_t item_type, zbitl::ByteView payload) {
  switch (item_type) {
    case ZBI_TYPE_CPU_TOPOLOGY: {
      if (payload.empty()) {
        return fit::error("ZBI_TYPE_CPU_TOPOLOGY payload is empty");
      }
      if (payload.size_bytes() % sizeof(zbi_topology_node_t) != 0) {
        return fit::error("ZBI_TYPE_CPU_TOPOLOGY payload not a multiple of entry size");
      }
      CpuTopologyTable result;
      result.table_ = std::span{
          reinterpret_cast<const zbi_topology_node_t*>(payload.data()),
          payload.size_bytes() / sizeof(zbi_topology_node_t),
      };
      return fit::ok(result);
    }
    case ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2: {
      if (payload.empty()) {
        return fit::error("ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2 payload is empty");
      }
      if (payload.size_bytes() % sizeof(zbi_topology_node_v2_t) != 0) {
        return fit::error(
            "ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2 payload not a multiple of entry size");
      }

      CpuTopologyTable result;
      result.table_ = std::span{
          reinterpret_cast<const zbi_topology_node_v2_t*>(payload.data()),
          payload.size_bytes() / sizeof(zbi_topology_node_v2_t),
      };
      return fit::ok(result);
    }
    case ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1:
      if (payload.size_bytes() >= sizeof(zbi_cpu_config_t)) {
        auto conf = reinterpret_cast<const zbi_cpu_config_t*>(payload.data());
        const size_t conf_size =
            sizeof(zbi_cpu_config_t) + (conf->cluster_count * sizeof(zbi_cpu_cluster_t));
        if (payload.size_bytes() < conf_size) {
          return fit::error("ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1 too small for cluster count");
        }
        CpuTopologyTable result;
        result.table_ = conf;
        return fit::ok(result);
      }
      return fit::error("ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1 too small for header");

    default:
      return fit::error("invalid ZBI item type for CpuTopologyTable");
  }
}

// These functions are static in the private inner class rather than just being
// in an anonymous namespace local to this file just so that the public classes
// can have a friend declaration.

struct CpuTopologyTable::Dispatch {
  // Set up with the modern table format, just use the input as is.

  static size_t TableSize(std::span<const zbi_topology_node_t> nodes) { return nodes.size(); }

  static iterator TableBegin(std::span<const zbi_topology_node_t> nodes) {
    iterator result;
    result.it_ = nodes.begin();
    return result;
  }

  static iterator TableEnd(std::span<const zbi_topology_node_t> nodes) {
    iterator result;
    result.it_ = nodes.end();
    return result;
  }

  static size_t TableSize(std::span<const zbi_topology_node_v2_t> nodes) { return nodes.size(); }

  static iterator TableBegin(std::span<const zbi_topology_node_v2_t> nodes) {
    V2ConvertingIterator it;
    it.v2_nodes_ = nodes;
    it.idx_ = 0;

    iterator result;
    result.it_ = it;
    return result;
  }

  static iterator TableEnd(std::span<const zbi_topology_node_v2_t> nodes) {
    V2ConvertingIterator it;
    it.v2_nodes_ = nodes;
    it.idx_ = nodes.size();

    iterator result;
    result.it_ = it;
    return result;
  }

  // Set up with the old table format, convert on the fly.

  static size_t TableSize(const zbi_cpu_config_t* config) {
    size_t nodes = 0;
    std::span clusters(config->clusters, config->cluster_count);
    for (const zbi_cpu_cluster_t& cluster : clusters) {
      // There's a node for the cluster, then a node for each CPU.
      nodes += 1 + cluster.cpu_count;
    }
    return nodes;
  }

  static iterator TableBegin(const zbi_cpu_config_t* config) {
    V1ConvertingIterator it;
    if (config->cluster_count > 0) {
      it.clusters_ = std::span(config->clusters, config->cluster_count);
      it.logical_id_ = 0;
    }
    iterator result;
    result.it_ = it;
    return result;
  }

  static iterator TableEnd(const zbi_cpu_config_t* config) {
    iterator result;
    result.it_ = V1ConvertingIterator();
    return result;
  }

  static void Advance(V1ConvertingIterator& it) {
    ZX_ASSERT_MSG(it.logical_id_, "cannot increment default-constructed or end iterator");
    ++it.next_node_idx_;

    if (!it.cpu_idx_) {
      // This is at the node for a cluster.  Advance to its first CPU.
      it.cpu_idx_ = 0;
      return;
    }

    const uint32_t cpu_count = it.clusters_[it.cluster_idx_].cpu_count;
    if (const uint32_t cpu_idx = *it.cpu_idx_; cpu_idx < cpu_count) {
      ++*it.logical_id_;
      ++*it.cpu_idx_;
      // If there are still CPUs to process, advance to the next one; else,
      // fall through to advance to the next cluster.
      if (cpu_idx < cpu_count - 1) {
        return;
      }
    }

    // Advance to the next cluster, unless we have reached the end.
    it.cluster_node_idx_ = it.next_node_idx_;
    it.cpu_idx_ = std::nullopt;
    if (++it.cluster_idx_ == it.clusters_.size()) {
      it.logical_id_ = std::nullopt;
    }
  }

  static zbi_topology_node_t GetNode(const V1ConvertingIterator& it) {
    ZX_ASSERT_MSG(it.logical_id_, "cannot dereference default-constructed or end iterator");

    // First there's a node for the cluster itself.
    if (!it.cpu_idx_) {
      return zbi_topology_node_t{
          // We don't have this data so it is a guess that little cores are
          // first.
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster = {.performance_class = it.cluster_idx_},
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      };
    }

    // Then there's a node for each CPU.
    return zbi_topology_node_t{
        .entity =
            {
                .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                .processor =
                    {
                        .architecture_info =
                            {
                                .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_ARM64,
                                .arm64 =
                                    {
                                        .cluster_1_id = it.cluster_idx_,
                                        .cpu_id = *it.cpu_idx_,
                                        .gic_id = *it.logical_id_,
                                    },
                            },
                        .logical_ids = {*it.logical_id_},
                        .logical_id_count = 1,
                    },
            },
        .parent_index = static_cast<uint16_t>(it.cluster_node_idx_),
    };
  }

  static zbi_topology_node_t GetNode(const V2ConvertingIterator& it) {
    ZX_ASSERT_MSG(it.idx_, "cannot dereference default-constructed iterator");
    const zbi_topology_node_v2_t& v2_node = it.v2_nodes_[*it.idx_];

    zbi_topology_node_t node = {.parent_index = v2_node.parent_index};
    zbi_topology_entity_t& entity = node.entity;
    auto& v2_entity = v2_node.entity;
    switch (v2_node.entity_type) {
      case ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR: {
        entity.discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR;
        entity.processor = zbi_topology_processor_t{
            .flags = v2_entity.processor.flags,
            .logical_id_count = v2_entity.processor.logical_id_count,

        };
        zbi_topology_architecture_info_t& arch_info = entity.processor.architecture_info;
        switch (v2_entity.processor.architecture) {
          case ZBI_TOPOLOGY_ARCHITECTURE_V2_X64:
            arch_info.discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_X64;
            arch_info.x64 = v2_entity.processor.architecture_info.x64;
            break;
          case ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64:
            arch_info.discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_ARM64;
            arch_info.arm64 = v2_entity.processor.architecture_info.arm64;
            break;
          case ZBI_TOPOLOGY_ARCHITECTURE_V2_RISCV64:
            arch_info.discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64;
            arch_info.riscv64 = v2_entity.processor.architecture_info.riscv64;
            break;
        }
        memcpy(entity.processor.logical_ids, v2_entity.processor.logical_ids,
               sizeof(entity.processor.logical_ids));
        break;
      }
      case ZBI_TOPOLOGY_ENTITY_V2_CLUSTER:
        entity.discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER;
        entity.cluster = v2_entity.cluster;
        break;
      case ZBI_TOPOLOGY_ENTITY_V2_CACHE:
        entity.discriminant = ZBI_TOPOLOGY_ENTITY_CACHE;
        entity.cache = v2_entity.cache;
        break;
      case ZBI_TOPOLOGY_ENTITY_V2_NUMA_REGION:
        entity.discriminant = ZBI_TOPOLOGY_ENTITY_NUMA_REGION;
        entity.numa_region = zbi_topology_numa_region_t{
            .start = v2_entity.numa_region.start_address,
            .size = v2_entity.numa_region.end_address - v2_entity.numa_region.start_address,
        };
        break;
    }
    return node;
  }
};

size_t CpuTopologyTable::size() const {
  return std::visit([](const auto& table) { return Dispatch::TableSize(table); }, table_);
}

CpuTopologyTable::iterator CpuTopologyTable::begin() const {
  return std::visit([](const auto& table) { return Dispatch::TableBegin(table); }, table_);
}

CpuTopologyTable::iterator CpuTopologyTable::end() const {
  return std::visit([](const auto& table) { return Dispatch::TableEnd(table); }, table_);
}

CpuTopologyTable::V1ConvertingIterator& CpuTopologyTable::V1ConvertingIterator::operator++() {
  Dispatch::Advance(*this);
  return *this;
}

zbi_topology_node_t CpuTopologyTable::V1ConvertingIterator::operator*() const {
  return Dispatch::GetNode(*this);
}

zbi_topology_node_t CpuTopologyTable::V2ConvertingIterator::operator*() const {
  return Dispatch::GetNode(*this);
}

}  // namespace zbitl
