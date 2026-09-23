// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/boot-shim/testing/devicetree-test-fixture.h"

#include <cstddef>

#include <zxtest/zxtest.h>

namespace boot_shim::testing {

std::optional<LoadedDtb> SyntheticDevicetreeTest::empty_dtb_ = std::nullopt;
std::optional<LoadedDtb> SyntheticDevicetreeTest::arm_gic2_no_msi_ = std::nullopt;
std::optional<LoadedDtb> SyntheticDevicetreeTest::arm_gic3_stride_ = std::nullopt;
std::optional<LoadedDtb> SyntheticDevicetreeTest::arm_gic3_four_stride_ = std::nullopt;
std::optional<LoadedDtb> SyntheticDevicetreeTest::arm_gic3_subsumed_stride_ = std::nullopt;

std::optional<LoadedDtb> ArmDevicetreeTest::crosvm_arm_ = std::nullopt;
std::optional<LoadedDtb> ArmDevicetreeTest::qemu_arm_gic3_ = std::nullopt;
std::optional<LoadedDtb> ArmDevicetreeTest::qemu_arm_gic2_ = std::nullopt;
std::optional<LoadedDtb> ArmDevicetreeTest::khadas_vim3_ = std::nullopt;

std::optional<LoadedDtb> RiscvDevicetreeTest::qemu_riscv_ = std::nullopt;
std::optional<LoadedDtb> RiscvDevicetreeTest::sifive_hifive_unmatched_ = std::nullopt;
std::optional<LoadedDtb> RiscvDevicetreeTest::vision_five_2_ = std::nullopt;
std::optional<LoadedDtb> RiscvDevicetreeTest::banana_pi_f3_ = std::nullopt;

void CheckCpuTopology(std::span<const zbi_topology_node_t> actual_nodes,
                      std::span<const zbi_topology_node_t> expected_nodes) {
  ASSERT_EQ(actual_nodes.size(), expected_nodes.size());
  for (size_t i = 0; i < actual_nodes.size(); ++i) {
    const auto& actual_node = actual_nodes[i];
    const auto& expected_node = expected_nodes[i];
    EXPECT_EQ(actual_node.parent_index, expected_node.parent_index);
    EXPECT_EQ(actual_node.entity.discriminant, expected_node.entity.discriminant);

    switch (actual_nodes[i].entity.discriminant) {
      case ZBI_TOPOLOGY_ENTITY_CLUSTER:
        EXPECT_EQ(actual_node.entity.cluster.performance_class,
                  expected_node.entity.cluster.performance_class);
        break;
      case ZBI_TOPOLOGY_ENTITY_SOCKET:
        break;
      case ZBI_TOPOLOGY_ENTITY_PROCESSOR:
        const auto& actual_processor = actual_node.entity.processor;
        const auto& expected_processor = expected_node.entity.processor;
        EXPECT_EQ(actual_processor.flags, expected_processor.flags);
        EXPECT_EQ(actual_processor.logical_id_count, expected_processor.logical_id_count);
        EXPECT_EQ(actual_processor.logical_ids[0], expected_processor.logical_ids[0]);
        EXPECT_EQ(actual_processor.logical_ids[1], expected_processor.logical_ids[1]);
        EXPECT_EQ(actual_processor.logical_ids[2], expected_processor.logical_ids[2]);
        EXPECT_EQ(actual_processor.logical_ids[3], expected_processor.logical_ids[3]);
        EXPECT_EQ(actual_processor.architecture_info.discriminant,
                  expected_processor.architecture_info.discriminant);
        switch (actual_nodes[i].entity.processor.architecture_info.discriminant) {
          case ZBI_TOPOLOGY_ARCHITECTURE_INFO_ARM64: {
            const auto& actual_arm64 = actual_processor.architecture_info.arm64;
            const auto& expected_arm64 = expected_processor.architecture_info.arm64;
            EXPECT_EQ(actual_arm64.cluster_1_id, expected_arm64.cluster_1_id);
            EXPECT_EQ(actual_arm64.cluster_2_id, expected_arm64.cluster_2_id);
            EXPECT_EQ(actual_arm64.cluster_3_id, expected_arm64.cluster_3_id);
            EXPECT_EQ(actual_arm64.cpu_id, expected_arm64.cpu_id);
            EXPECT_EQ(actual_arm64.gic_id, expected_arm64.gic_id);
          } break;
          case ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64: {
            const auto& actual_riscv = actual_processor.architecture_info.riscv64;
            const auto& expected_riscv = expected_processor.architecture_info.riscv64;
            EXPECT_EQ(actual_riscv.hart_id, expected_riscv.hart_id);
            EXPECT_EQ(actual_riscv.isa_strtab_index, expected_riscv.isa_strtab_index);
          } break;
          default:
            break;
        }

        break;
    }
  }
}

void CheckMmioRanges(std::span<const boot_shim::MmioRange> actual,
                     std::span<const boot_shim::MmioRange> expected) {
  size_t matched_count = 0;

  for (size_t i = 0; i < expected.size(); ++i) {
    const auto& expected_range = expected[i];

    bool matched = false;
    for (size_t j = 0; j < actual.size(); ++j) {
      const auto& actual_range = actual[j];

      if (actual_range.address != expected_range.address) {
        continue;
      }

      if (actual_range.size != expected_range.size) {
        continue;
      }

      matched = true;
      matched_count++;
    }
    EXPECT_TRUE(matched, "Expected range %zu [0x%zx, 0x%zx] had no match.", i,
                static_cast<size_t>(expected_range.address),
                static_cast<size_t>(expected_range.end()));
  }

  EXPECT_EQ(matched_count, actual.size(), "There were more actual_ranges than expected ranges.");
}

}  // namespace boot_shim::testing
