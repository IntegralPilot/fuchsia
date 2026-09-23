// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree-boot-shim.h>
#include <lib/boot-shim/devicetree.h>
#include <lib/boot-shim/testing/devicetree-test-fixture.h>
#include <lib/fit/defer.h>
#include <lib/zbi-format/cpu.h>
#include <lib/zbitl/image.h>

#include <fbl/alloc_checker.h>
#include <zxtest/zxtest.h>

namespace {

using boot_shim::testing::LoadDtb;
using boot_shim::testing::LoadedDtb;

template <uint64_t Value>
struct MockArmMpidr {
  static arch::ArmMultiprocessorAffinityRegister Read() {
    arch::ArmMultiprocessorAffinityRegister mpidr;
    mpidr.set_reg_value(Value);
    return mpidr;
  }
};

template <uint64_t BootMpidr>
using ArmDevicetreeCpuTopologyItem =
    boot_shim::ArmDevicetreeCpuTopologyItem<MockArmMpidr<BootMpidr>>;

class TestAllocator {
 public:
  TestAllocator() = default;
  TestAllocator(TestAllocator&& other) {
    allocs_ = std::move(other.allocs_);
    other.allocs_.clear();
  }

  ~TestAllocator() {
    for (auto* alloc : allocs_) {
      free(alloc);
    }
  }

  void* operator()(size_t size, size_t alignment, fbl::AllocChecker& ac) {
    void* alloc = malloc(size + alignment);
    allocs_.push_back(alloc);
    ac.arm(size + alignment, alloc != nullptr);
    return reinterpret_cast<void*>((reinterpret_cast<uintptr_t>(alloc) + alignment) &
                                   ~(alignment - 1));
  }

 private:
  std::vector<void*> allocs_;
};

class ArmCpuTopologyItemTest
    : public boot_shim::testing::TestMixin<boot_shim::testing::ArmDevicetreeTest> {
 public:
  static void SetUpTestSuite() {
    Mixin::SetUpTestSuite();
    auto loaded_dtb = LoadDtb("cpus_arm.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    cpus_dtb_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("cpus_arm_no_cpu_map.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    cpus_no_cpu_map_dtb_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("cpus_arm_single_cell.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    cpus_single_cell_dtb_ = std::move(loaded_dtb).value();
  }
  static void TearDownTestSuite() {
    cpus_dtb_ = std::nullopt;
    cpus_no_cpu_map_dtb_ = std::nullopt;
    cpus_single_cell_dtb_ = std::nullopt;
    Mixin::TearDownTestSuite();
  }

  devicetree::Devicetree cpus() { return cpus_dtb_->fdt(); }
  devicetree::Devicetree cpus_single_cell() { return cpus_single_cell_dtb_->fdt(); }
  devicetree::Devicetree cpus_no_cpu_map() { return cpus_no_cpu_map_dtb_->fdt(); }

 private:
  static std::optional<LoadedDtb> cpus_dtb_;
  static std::optional<LoadedDtb> cpus_single_cell_dtb_;
  static std::optional<LoadedDtb> cpus_no_cpu_map_dtb_;
};

std::optional<LoadedDtb> ArmCpuTopologyItemTest::cpus_dtb_ = std::nullopt;
std::optional<LoadedDtb> ArmCpuTopologyItemTest::cpus_single_cell_dtb_ = std::nullopt;
std::optional<LoadedDtb> ArmCpuTopologyItemTest::cpus_no_cpu_map_dtb_ = std::nullopt;

TEST_F(ArmCpuTopologyItemTest, CpusMultipleCells) {
  constexpr std::array kExpectedTopology = {

      // socket0
      zbi_topology_node_t{
          .entity = {.discriminant = ZBI_TOPOLOGY_ENTITY_SOCKET},
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },

      // cluster0
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster =
                      {
                          .performance_class = 0x7F,
                      },
              },
          .parent_index = 0,
      },

      // cpu@11100000101
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 7,
                                          .cpu_id = 1,
                                          .gic_id = 0,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {3, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 1,
      },

      // cpu@11100000100
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 7,
                                          .cpu_id = 0,
                                          .gic_id = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {1, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 1,
      },

      // cluster1
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster =
                      {
                          .performance_class = 0xFF,
                      },
              },
          .parent_index = 0,
      },

      // cpu@1
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 1,
                                          .gic_id = 2,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {2, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 4,
      },

      // cpu@0
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 3,
                                      },
                              },
                          .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
                          .logical_ids = {0, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 4,
      },
  };

  std::array<std::byte, 1024> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = cpus();
  boot_shim::DevicetreeBootShim<ArmDevicetreeCpuTopologyItem<0>> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  shim.Init();
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_CPU_TOPOLOGY) {
      present = true;
      std::span<zbi_topology_node_t> nodes(reinterpret_cast<zbi_topology_node_t*>(payload.data()),
                                           payload.size() / sizeof(zbi_topology_node_t));
      boot_shim::testing::CheckCpuTopology(nodes, kExpectedTopology);
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(ArmCpuTopologyItemTest, CpusSingleCell) {
  constexpr std::array kExpectedTopology = {

      // socket0
      zbi_topology_node_t{
          .entity = {.discriminant = ZBI_TOPOLOGY_ENTITY_SOCKET},
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },

      // cluster0
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster =
                      {
                          .performance_class = 0x7F,
                      },
              },
          .parent_index = 0,
      },

      // cpu@101
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 1,
                                          .gic_id = 0,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {3, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 1,
      },

      // cpu@100
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {1, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 1,
      },

      // cluster1
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster =
                      {
                          .performance_class = 0xFF,
                      },
              },
          .parent_index = 0,
      },

      // cpu@1
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 1,
                                          .gic_id = 2,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {2, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 4,
      },

      // cpu@0
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 3,
                                      },
                              },
                          .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
                          .logical_ids = {0, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 4,
      },
  };

  std::array<std::byte, 1024> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = cpus_single_cell();
  boot_shim::DevicetreeBootShim<ArmDevicetreeCpuTopologyItem<0>> shim("test", fdt);
  shim.set_allocator(TestAllocator());
  shim.Init();
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_CPU_TOPOLOGY) {
      present = true;
      std::span<zbi_topology_node_t> nodes(reinterpret_cast<zbi_topology_node_t*>(payload.data()),
                                           payload.size() / sizeof(zbi_topology_node_t));
      boot_shim::testing::CheckCpuTopology(nodes, kExpectedTopology);
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(ArmCpuTopologyItemTest, CpusNoCpuMap) {
  constexpr std::array kExpectedTopology = {

      // cpu@0
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 0,
                                      },
                              },
                          .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
                          .logical_ids = {0, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },

      // cpu@1
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 1,
                                          .gic_id = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {1, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },

      // cpu@100
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 2,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {2, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },

      // cpu@0
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 1,
                                          .gic_id = 3,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {3, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },
  };

  std::array<std::byte, 1024> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = cpus_no_cpu_map();
  boot_shim::DevicetreeBootShim<ArmDevicetreeCpuTopologyItem<0>> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  shim.Init();
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_CPU_TOPOLOGY) {
      present = true;
      std::span<zbi_topology_node_t> nodes(reinterpret_cast<zbi_topology_node_t*>(payload.data()),
                                           payload.size() / sizeof(zbi_topology_node_t));
      boot_shim::testing::CheckCpuTopology(nodes, kExpectedTopology);
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(ArmCpuTopologyItemTest, CpusBootCpuWithNonZeroMpidr) {
  constexpr uint64_t kBootMpidr = 0x700000101;  // Matches cpu@11100000101
  constexpr std::array kExpectedTopology = {

      // socket0
      zbi_topology_node_t{
          .entity = {.discriminant = ZBI_TOPOLOGY_ENTITY_SOCKET},
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },

      // cluster0
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster =
                      {
                          .performance_class = 0x7F,
                      },
              },
          .parent_index = 0,
      },

      // cpu@11100000101
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 7,
                                          .cpu_id = 1,
                                          .gic_id = 0,
                                      },
                              },
                          .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
                          .logical_ids = {0, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 1,
      },

      // cpu@11100000100
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 7,
                                          .cpu_id = 0,
                                          .gic_id = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {1, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 1,
      },

      // cluster1
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster =
                      {
                          .performance_class = 0xFF,
                      },
              },
          .parent_index = 0,
      },

      // cpu@1
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 1,
                                          .gic_id = 2,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {2, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 4,
      },

      // cpu@0
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 3,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {3, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 4,
      },
  };

  std::array<std::byte, 1024> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = cpus();
  boot_shim::DevicetreeBootShim<ArmDevicetreeCpuTopologyItem<kBootMpidr>> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  shim.Init();
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_CPU_TOPOLOGY) {
      present = true;
      std::span<zbi_topology_node_t> nodes(reinterpret_cast<zbi_topology_node_t*>(payload.data()),
                                           payload.size() / sizeof(zbi_topology_node_t));
      boot_shim::testing::CheckCpuTopology(nodes, kExpectedTopology);
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(ArmCpuTopologyItemTest, Qemu) {
  constexpr std::array kExpectedTopology = {

      // cpu@0
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 0,
                                      },
                              },
                          .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
                          .logical_ids = {0, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },

      // cpu@1
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 1,
                                          .gic_id = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {1, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },

      // cpu@100
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 2,
                                          .gic_id = 2,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {2, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },

      // cpu@0
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 3,
                                          .gic_id = 3,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {3, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },
  };

  std::array<std::byte, 1024> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = qemu_arm_gic3();
  boot_shim::DevicetreeBootShim<ArmDevicetreeCpuTopologyItem<0>> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  shim.Init();
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_CPU_TOPOLOGY) {
      present = true;
      std::span<zbi_topology_node_t> nodes(reinterpret_cast<zbi_topology_node_t*>(payload.data()),
                                           payload.size() / sizeof(zbi_topology_node_t));
      boot_shim::testing::CheckCpuTopology(nodes, kExpectedTopology);
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(ArmCpuTopologyItemTest, Crosvm) {
  constexpr std::array kExpectedTopology = {

      // cpu@0
      zbi_topology_node_t{
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
                                          .cluster_2_id = 0,
                                          .cluster_3_id = 0,
                                          .cpu_id = 0,
                                          .gic_id = 0,
                                      },
                              },
                          .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
                          .logical_ids = {0, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      }};

  std::array<std::byte, 1024> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = crosvm_arm();
  boot_shim::DevicetreeBootShim<ArmDevicetreeCpuTopologyItem<0>> shim("test", fdt);
  shim.set_allocator(TestAllocator());
  shim.Init();
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_CPU_TOPOLOGY) {
      present = true;
      std::span<zbi_topology_node_t> nodes(reinterpret_cast<zbi_topology_node_t*>(payload.data()),
                                           payload.size() / sizeof(zbi_topology_node_t));
      boot_shim::testing::CheckCpuTopology(nodes, kExpectedTopology);
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(ArmCpuTopologyItemTest, KhadasVim3) {
  constexpr std::array
      kExpectedTopology =
          {

              // cluster0
              zbi_topology_node_t{
                  .entity =
                      {
                          .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                          .cluster =
                              {
                                  .performance_class = 0x93,
                              },
                      },
                  .parent_index = ZBI_TOPOLOGY_NO_PARENT,
              },

              // cpu@0
              zbi_topology_node_t{
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
                                                  .cluster_2_id = 0,
                                                  .cluster_3_id = 0,
                                                  .cpu_id = 0,
                                                  .gic_id = 0,
                                              },
                                      },
                                  .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
                                  .logical_ids = {0, 0, 0, 0},
                                  .logical_id_count = 1,
                              },
                      },
                  .parent_index = 0,
              },

              // core@1
              zbi_topology_node_t{
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
                                                  .cluster_2_id = 0,
                                                  .cluster_3_id = 0,
                                                  .cpu_id = 1,
                                                  .gic_id = 1,
                                              },
                                      },
                                  .flags = 0,
                                  .logical_ids = {1, 0, 0, 0},
                                  .logical_id_count = 1,
                              },
                      },
                  .parent_index = 0,
              },

              // cluster1
              zbi_topology_node_t{
                  .entity =
                      {
                          .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                          .cluster =
                              {
                                  .performance_class = 0xFF,
                              },
                      },
                  .parent_index = ZBI_TOPOLOGY_NO_PARENT,
              },

              // core@100
              zbi_topology_node_t{
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
                                                  .cluster_2_id = 0,
                                                  .cluster_3_id = 0,
                                                  .cpu_id = 0,
                                                  .gic_id = 2,
                                              },
                                      },
                                  .flags = 0,
                                  .logical_ids = {2, 0, 0, 0},
                                  .logical_id_count = 1,
                              },
                      },
                  .parent_index = 3,
              },

              // core@101
              zbi_topology_node_t{
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
                                                  .cluster_2_id = 0,
                                                  .cluster_3_id = 0,
                                                  .cpu_id = 1,
                                                  .gic_id = 3,
                                              },
                                      },
                                  .flags = 0,
                                  .logical_ids = {3, 0, 0, 0},
                                  .logical_id_count = 1,
                              },
                      },
                  .parent_index = 3,
              },

              // core@102
              zbi_topology_node_t{
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
                                                  .cluster_2_id = 0,
                                                  .cluster_3_id = 0,
                                                  .cpu_id = 2,
                                                  .gic_id = 4,
                                              },
                                      },
                                  .flags = 0,
                                  .logical_ids = {4, 0, 0, 0},
                                  .logical_id_count = 1,
                              },
                      },
                  .parent_index = 3,
              },

              // core@103
              zbi_topology_node_t{
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
                                                  .cluster_2_id = 0,
                                                  .cluster_3_id = 0,
                                                  .cpu_id = 3,
                                                  .gic_id = 5,
                                              },
                                      },
                                  .flags = 0,
                                  .logical_ids = {5, 0, 0, 0},
                                  .logical_id_count = 1,
                              },
                      },
                  .parent_index = 3,
              },
          };

  std::array<std::byte, 1024> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = khadas_vim3();
  boot_shim::DevicetreeBootShim<ArmDevicetreeCpuTopologyItem<0>> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  shim.Init();
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_CPU_TOPOLOGY) {
      present = true;
      std::span<zbi_topology_node_t> nodes(reinterpret_cast<zbi_topology_node_t*>(payload.data()),
                                           payload.size() / sizeof(zbi_topology_node_t));
      boot_shim::testing::CheckCpuTopology(nodes, kExpectedTopology);
    }
  }
  ASSERT_TRUE(present);
}

}  // namespace
