// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zbitl/items/cpu-topology.h>
#include <lib/zbitl/storage-traits.h>

#include <algorithm>
#include <iterator>
#include <memory>
#include <span>
#include <string_view>
#include <vector>

#include <gtest/gtest.h>

#include "lib/zbi-format/cpu.h"

namespace {

class CpuTopologyPayload {
 public:
  explicit CpuTopologyPayload(std::span<const zbi_topology_node_t> nodes) : nodes_(nodes) {}

  std::span<const std::byte> as_bytes() const { return std::as_bytes(nodes_); }

 private:
  std::span<const zbi_topology_node_t> nodes_;
};

class CpuTopologyV1Payload {
 public:
  explicit CpuTopologyV1Payload(std::span<const zbi_cpu_cluster_t> clusters) {
    const zbi_cpu_config_t config = {
        .cluster_count = static_cast<uint32_t>(clusters.size()),
    };
    for (std::span<const std::byte> chunk : {
             std::as_bytes(std::span(&config, 1)),
             std::as_bytes(clusters),
         }) {
      data_.insert(data_.end(), chunk.begin(), chunk.end());
    }
  }

  std::span<const std::byte> as_bytes() const { return {data_}; }

 private:
  std::vector<std::byte> data_;
};

class CpuTopologyV2Payload {
 public:
  explicit CpuTopologyV2Payload(std::span<const zbi_topology_node_v2_t> nodes) : nodes_(nodes) {}

  std::span<const std::byte> as_bytes() const { return std::as_bytes(nodes_); }

 private:
  std::span<const zbi_topology_node_v2_t> nodes_;
};

void ExpectArmNodesAreEqual(const zbi_topology_node_t& expected_node,
                            const zbi_topology_node_t& actual_node) {
  ASSERT_EQ(expected_node.entity.discriminant, actual_node.entity.discriminant);
  EXPECT_EQ(expected_node.parent_index, actual_node.parent_index);
  switch (actual_node.entity.discriminant) {
    case ZBI_TOPOLOGY_ENTITY_CLUSTER: {
      const zbi_topology_cluster_t& actual = actual_node.entity.cluster;
      const zbi_topology_cluster_t& expected = expected_node.entity.cluster;
      EXPECT_EQ(expected.performance_class, actual.performance_class);
      break;
    }
    case ZBI_TOPOLOGY_ENTITY_PROCESSOR: {
      const zbi_topology_processor_t& actual = actual_node.entity.processor;
      const zbi_topology_processor_t& expected = expected_node.entity.processor;
      ASSERT_EQ(expected.logical_id_count, actual.logical_id_count);
      for (size_t j = 0; j < actual.logical_id_count; ++j) {
        EXPECT_EQ(expected.logical_ids[j], actual.logical_ids[j]) << "logical_ids[" << j << "])";
      }
      EXPECT_EQ(actual.flags, expected.flags);
      ASSERT_EQ(actual.architecture_info.discriminant, expected.architecture_info.discriminant);
      switch (actual.architecture_info.discriminant) {
        case ZBI_TOPOLOGY_ARCHITECTURE_INFO_ARM64: {
          const zbi_topology_arm64_info_t& actual_info = actual.architecture_info.arm64;
          const zbi_topology_arm64_info_t& expected_info = expected.architecture_info.arm64;
          EXPECT_EQ(expected_info.cluster_1_id, actual_info.cluster_1_id);
          EXPECT_EQ(expected_info.cluster_2_id, actual_info.cluster_2_id);
          EXPECT_EQ(expected_info.cluster_3_id, actual_info.cluster_3_id);
          EXPECT_EQ(expected_info.cpu_id, actual_info.cpu_id);
          EXPECT_EQ(expected_info.gic_id, actual_info.gic_id);
          break;
        }
      }
      break;
    }
    case ZBI_TOPOLOGY_ENTITY_NUMA_REGION: {
      const zbi_topology_numa_region_t& actual = actual_node.entity.numa_region;
      const zbi_topology_numa_region_t& expected = expected_node.entity.numa_region;
      EXPECT_EQ(expected.start, actual.start);
      EXPECT_EQ(expected.size, actual.size);
    }
  }
}

void ExpectTableHasArmNodes(const zbitl::CpuTopologyTable& table,
                            std::span<const zbi_topology_node_t> nodes) {
  EXPECT_EQ(table.size(), nodes.size());
  EXPECT_EQ(table.size(), static_cast<size_t>(std::distance(table.begin(), table.end())));
  auto it = table.begin();
  for (size_t i = 0; i < nodes.size(); ++i, ++it) {
    const zbi_topology_node_t& expected = nodes[i];
    const zbi_topology_node_t actual = *it;
    ASSERT_NO_FATAL_FAILURE(ExpectArmNodesAreEqual(expected, actual)) << i;
  }
}

TEST(CpuTopologyTableTests, BadType) {
  CpuTopologyV1Payload payload({});
  auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_DISCARD, payload.as_bytes());
  ASSERT_TRUE(result.is_error());
  std::string_view error = std::move(result).error_value();
  EXPECT_EQ("invalid ZBI item type for CpuTopologyTable", error);
}

TEST(CpuTopologyTableTests, NoCores) {
  // CONFIG: empty payload.
  {
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1, {});
    ASSERT_TRUE(result.is_error());
    std::string_view error = std::move(result).error_value();
    EXPECT_EQ("ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1 too small for header", error);
  }

  // CONFIG: empty cluster.
  {
    CpuTopologyV1Payload payload({});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1,
                                                       payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {}));
  }

  // TOPOLOGY: empty payload.
  {
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2, {});
    ASSERT_TRUE(result.is_error());
    std::string_view error = std::move(result).error_value();
    EXPECT_EQ("ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2 payload is empty", error);
  }
}

TEST(CpuTopologyTableTests, SingleArmCore) {
  constexpr zbi_cpu_cluster_t kConfig[] = {{.cpu_count = 1}};
  constexpr zbi_topology_node_v2_t kV2Nodes[] = {
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_CLUSTER,
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
          .entity = {.cluster = {.performance_class = 0}},
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 0,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {0},
                          .logical_id_count = 1,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 0,
                                      },
                              },
                      },
              },
      },
  };
  constexpr zbi_topology_node_t kNodes[] = {
      {
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster = {.performance_class = 0},
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },
      {

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
                                          .cluster_1_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 0,
                                      },
                              },
                          .logical_ids = {0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },
  };

  {
    CpuTopologyV1Payload payload({kConfig});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1,
                                                       payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
  {
    CpuTopologyV2Payload payload({kV2Nodes});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2,
                                                       payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
  {
    CpuTopologyPayload payload({kNodes});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_CPU_TOPOLOGY, payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
}

TEST(CpuTopologyTableTests, TwoArmCoresAcrossOneCluster) {
  constexpr zbi_cpu_cluster_t kConfig[] = {{.cpu_count = 2}};
  constexpr zbi_topology_node_v2_t kV2Nodes[] = {
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_CLUSTER,
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
          .entity = {.cluster = {.performance_class = 0}},
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 0,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {0},
                          .logical_id_count = 1,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 0,
                                      },
                              },
                      },
              },
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 0,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {1},
                          .logical_id_count = 1,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 0,
                                          .cpu_id = 1,
                                          .gic_id = 1,
                                      },
                              },
                      },
              },
      },
  };
  constexpr zbi_topology_node_t kNodes[] = {
      {
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster = {.performance_class = 0},
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },
      {
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
                                          .cluster_1_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 0,
                                      },
                              },
                          .logical_ids = {0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },
      {

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
                                          .cluster_1_id = 0,
                                          .cpu_id = 1,
                                          .gic_id = 1,
                                      },
                              },
                          .logical_ids = {1},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },
  };

  {
    CpuTopologyV1Payload payload({kConfig});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1,
                                                       payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
  {
    CpuTopologyV2Payload payload({kV2Nodes});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2,
                                                       payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
  {
    CpuTopologyPayload payload({kNodes});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_CPU_TOPOLOGY, payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
}

TEST(CpuTopologyTableTests, FourArmCoresAcrossOneCluster) {
  constexpr zbi_cpu_cluster_t kConfig[] = {{.cpu_count = 4}};
  constexpr zbi_topology_node_v2_t kV2Nodes[] = {
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_CLUSTER,
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
          .entity = {.cluster = {.performance_class = 0}},
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 0,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {0},
                          .logical_id_count = 1,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 0,
                                      },
                              },
                      },
              },
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 0,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {1},
                          .logical_id_count = 1,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 0,
                                          .cpu_id = 1,
                                          .gic_id = 1,
                                      },
                              },
                      },
              },
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 0,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {2},
                          .logical_id_count = 1,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 0,
                                          .cpu_id = 2,
                                          .gic_id = 2,
                                      },
                              },
                      },
              },
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 0,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {3},
                          .logical_id_count = 1,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 0,
                                          .cpu_id = 3,
                                          .gic_id = 3,
                                      },
                              },
                      },
              },
      },
  };
  constexpr zbi_topology_node_t kNodes[] = {
      {

          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster = {.performance_class = 0},
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },
      {

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
                                          .cluster_1_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 0,
                                      },
                              },
                          .logical_ids = {0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },
      {

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
                                          .cluster_1_id = 0,
                                          .cpu_id = 1,
                                          .gic_id = 1,
                                      },
                              },
                          .logical_ids = {1},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },
      {

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
                                          .cluster_1_id = 0,
                                          .cpu_id = 2,
                                          .gic_id = 2,
                                      },
                              },
                          .logical_ids = {2},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },
      {

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
                                          .cluster_1_id = 0,
                                          .cpu_id = 3,
                                          .gic_id = 3,
                                      },
                              },
                          .logical_ids = {3},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },
  };

  {
    CpuTopologyV1Payload payload({kConfig});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1,
                                                       payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
  {
    CpuTopologyV2Payload payload({kV2Nodes});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2,
                                                       payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
  {
    CpuTopologyPayload payload({kNodes});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_CPU_TOPOLOGY, payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
}

TEST(CpuTopologyTableTests, TwoArmCoresAcrossTwoClusters) {
  constexpr zbi_cpu_cluster_t kConfig[] = {{.cpu_count = 1}, {.cpu_count = 1}};
  constexpr zbi_topology_node_v2_t kV2Nodes[] = {
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_CLUSTER,
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
          .entity = {.cluster = {.performance_class = 0}},
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 0,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {0},
                          .logical_id_count = 1,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 0,
                                      },
                              },
                      },
              },
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_CLUSTER,
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
          .entity = {.cluster = {.performance_class = 1}},
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 2,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {1},
                          .logical_id_count = 1,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 1,
                                          .cpu_id = 0,
                                          .gic_id = 1,
                                      },
                              },
                      },
              },
      },
  };
  constexpr zbi_topology_node_t kNodes[] = {
      {

          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster = {.performance_class = 0},
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },
      {

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
                                          .cluster_1_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 0,
                                      },
                              },
                          .logical_ids = {0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },
      {

          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster = {.performance_class = 1},
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },
      {

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
                                          .cluster_1_id = 1,
                                          .cpu_id = 0,
                                          .gic_id = 1,
                                      },
                              },
                          .logical_ids = {1},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 2,
      },
  };

  {
    CpuTopologyV1Payload payload({kConfig});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1,
                                                       payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
  {
    CpuTopologyV2Payload payload({kV2Nodes});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2,
                                                       payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
  {
    CpuTopologyPayload payload({kNodes});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_CPU_TOPOLOGY, payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
}

TEST(CpuTopologyTableTests, SixArmCoresAcrossThreeClusters) {
  constexpr zbi_cpu_cluster_t kConfig[] = {{.cpu_count = 1}, {.cpu_count = 3}, {.cpu_count = 2}};
  constexpr zbi_topology_node_v2_t kV2Nodes[] = {
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_CLUSTER,
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
          .entity = {.cluster = {.performance_class = 0}},
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 0,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {0},
                          .logical_id_count = 1,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 0,
                                      },
                              },
                      },
              },
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_CLUSTER,
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
          .entity = {.cluster = {.performance_class = 1}},
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 2,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {1},
                          .logical_id_count = 1,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 1,
                                          .cpu_id = 0,
                                          .gic_id = 1,
                                      },
                              },
                      },
              },
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 2,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {2},
                          .logical_id_count = 1,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 1,
                                          .cpu_id = 1,
                                          .gic_id = 2,
                                      },
                              },
                      },
              },
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 2,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {3},
                          .logical_id_count = 1,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 1,
                                          .cpu_id = 2,
                                          .gic_id = 3,
                                      },
                              },
                      },
              },
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_CLUSTER,
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
          .entity = {.cluster = {.performance_class = 2}},
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 6,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {4},
                          .logical_id_count = 1,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 2,
                                          .cpu_id = 0,
                                          .gic_id = 4,
                                      },
                              },
                      },
              },
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 6,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {5},
                          .logical_id_count = 1,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 2,
                                          .cpu_id = 1,
                                          .gic_id = 5,
                                      },
                              },
                      },
              },
      },
  };
  constexpr zbi_topology_node_t kNodes[] = {
      {

          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster = {.performance_class = 0},
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },
      {

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
                                          .cluster_1_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 0,
                                      },
                              },
                          .logical_ids = {0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },
      {

          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster = {.performance_class = 1},
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },
      {

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
                                          .cluster_1_id = 1,
                                          .cpu_id = 0,
                                          .gic_id = 1,
                                      },
                              },
                          .logical_ids = {1},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 2,
      },
      {

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
                                          .cluster_1_id = 1,
                                          .cpu_id = 1,
                                          .gic_id = 2,
                                      },
                              },
                          .logical_ids = {2},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 2,
      },
      {

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
                                          .cluster_1_id = 1,
                                          .cpu_id = 2,
                                          .gic_id = 3,
                                      },
                              },
                          .logical_ids = {3},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 2,
      },
      {

          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster = {.performance_class = 2},
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },
      {

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
                                          .cluster_1_id = 2,
                                          .cpu_id = 0,
                                          .gic_id = 4,
                                      },
                              },
                          .logical_ids = {4},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 6,
      },
      {

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
                                          .cluster_1_id = 2,
                                          .cpu_id = 1,
                                          .gic_id = 5,
                                      },
                              },
                          .logical_ids = {5},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 6,
      },
  };

  {
    CpuTopologyV1Payload payload({kConfig});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V1,
                                                       payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
  {
    CpuTopologyV2Payload payload({kV2Nodes});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2,
                                                       payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
  {
    CpuTopologyPayload payload({kNodes});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_CPU_TOPOLOGY, payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
}

TEST(CpuTopologyTableTests, Sherlock) {
  // The CPU topology of the Sherlock board.
  constexpr zbi_topology_node_v2_t kSherlockV2Nodes[] = {
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_CLUSTER,
          .parent_index = 0,
          .entity = {.cluster = {.performance_class = 0}},
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 0,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {0},
                          .logical_id_count = 1,
                          .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 0,
                                      },
                              },
                      },
              },
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 0,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {1},
                          .logical_id_count = 1,
                          .flags = 0,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 0,
                                          .cpu_id = 1,
                                          .gic_id = 1,
                                      },
                              },
                      },
              },
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_CLUSTER,
          .parent_index = 0,
          .entity = {.cluster = {.performance_class = 1}},
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 3,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {2},
                          .logical_id_count = 1,
                          .flags = 0,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 1,
                                          .cpu_id = 0,
                                          .gic_id = 4,
                                      },
                              },
                      },
              },
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 3,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {3},
                          .logical_id_count = 1,
                          .flags = 0,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 1,
                                          .cpu_id = 1,
                                          .gic_id = 5,
                                      },
                              },
                      },
              },
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 3,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {4},
                          .logical_id_count = 1,
                          .flags = 0,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 1,
                                          .cpu_id = 2,
                                          .gic_id = 6,
                                      },
                              },
                      },
              },
      },
      {
          .entity_type = ZBI_TOPOLOGY_ENTITY_V2_PROCESSOR,
          .parent_index = 3,
          .entity =
              {
                  .processor =
                      {
                          .logical_ids = {5},
                          .logical_id_count = 1,
                          .flags = 0,
                          .architecture = ZBI_TOPOLOGY_ARCHITECTURE_V2_ARM64,
                          .architecture_info =
                              {
                                  .arm64 =
                                      {
                                          .cluster_1_id = 1,
                                          .cpu_id = 3,
                                          .gic_id = 7,
                                      },
                              },
                      },
              },
      },
  };

  constexpr zbi_topology_node_t kNodes[] = {
      {

          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster = {.performance_class = 0},
              },
          .parent_index = 0,
      },
      {

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
                                          .cluster_1_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 0,
                                      },
                              },
                          .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
                          .logical_ids = {0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },
      {

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
                                          .cluster_1_id = 0,
                                          .cpu_id = 1,
                                          .gic_id = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {1},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },
      {

          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster = {.performance_class = 1},
              },
          .parent_index = 0,
      },
      {

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
                                          .cluster_1_id = 1,
                                          .cpu_id = 0,
                                          .gic_id = 4,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {2},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 3,
      },
      {

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
                                          .cluster_1_id = 1,
                                          .cpu_id = 1,
                                          .gic_id = 5,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {3},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 3,
      },
      {

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
                                          .cluster_1_id = 1,
                                          .cpu_id = 2,
                                          .gic_id = 6,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {4},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 3,
      },
      {

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
                                          .cluster_1_id = 1,
                                          .cpu_id = 3,
                                          .gic_id = 7,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {5},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 3,
      },
  };

  {
    CpuTopologyV2Payload payload({kSherlockV2Nodes});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_DEPRECATED_CPU_TOPOLOGY_V2,
                                                       payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
  {
    CpuTopologyPayload payload({kNodes});
    auto result = zbitl::CpuTopologyTable::FromPayload(ZBI_TYPE_CPU_TOPOLOGY, payload.as_bytes());
    ASSERT_FALSE(result.is_error()) << result.error_value();
    const auto table = std::move(result).value();
    ASSERT_NO_FATAL_FAILURE(ExpectTableHasArmNodes(table, {kNodes}));
  }
}

}  // namespace
