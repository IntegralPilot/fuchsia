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
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/image.h>

#include <cstdint>
#include <initializer_list>
#include <string>
#include <string_view>
#include <vector>

#include <zxtest/zxtest.h>
namespace {
using boot_shim::testing::LoadDtb;
using boot_shim::testing::LoadedDtb;

template <uint64_t BootHartId>
struct BootHartIdGetter {
  static uint64_t Get() { return BootHartId; }
};

template <uint64_t BootHartId>
using RiscvDevicetreeCpuTopologyItem =
    boot_shim::RiscvDevicetreeCpuTopologyItem<BootHartIdGetter<BootHartId>>;

// An ISA string common to multiple test devicetrees.
constexpr std::string_view kCommonTestHartIsaString =
    "rv64imafdch_zicsr_zifencei_zihintpause_zba_zbb_zbc_zbs_sstc";

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

// Initial NUL entry is automatically included.
std::string BuildStringTable(std::initializer_list<std::string_view> strings) {
  std::string table;
  table += '\0';  // Initial NUL.
  for (std::string_view sv : strings) {
    table += sv;
    table += '\0';
  }
  return table;
}

// Initial NUL entry among the expected strings is automatically assumed and
// should not be provided.
void ExpectStringTable(std::initializer_list<std::string_view> expected_strs,
                       std::span<const std::byte> actual) {
  std::string expected = BuildStringTable(expected_strs);
  ASSERT_EQ(expected.size(), actual.size());
  EXPECT_BYTES_EQ(expected.data(), actual.data(), actual.size());
}

class RiscvDevicetreeCpuTopologyItemTest
    : public boot_shim::testing::TestMixin<boot_shim::testing::RiscvDevicetreeTest,
                                           boot_shim::testing::SyntheticDevicetreeTest> {
 public:
  static void SetUpTestSuite() {
    Mixin::SetUpTestSuite();
    auto loaded_dtb = LoadDtb("cpus_riscv.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    riscv_cpus_dtb_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("cpus_riscv_nested_clusters.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    riscv_cpus_nested_clusters_dtb_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("cpus_no_cpu_map_riscv.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    riscv_cpus_no_cpu_map_dtb_ = std::move(loaded_dtb).value();
  }

  static void TearDownTestSuite() {
    riscv_cpus_dtb_ = std::nullopt;
    riscv_cpus_no_cpu_map_dtb_ = std::nullopt;
    Mixin::TearDownTestSuite();
  }

  devicetree::Devicetree riscv_cpus() { return riscv_cpus_dtb_->fdt(); }
  devicetree::Devicetree riscv_cpus_nested_clusters() {
    return riscv_cpus_nested_clusters_dtb_->fdt();
  }
  devicetree::Devicetree riscv_cpus_no_cpu_map() { return riscv_cpus_no_cpu_map_dtb_->fdt(); }

 private:
  static std::optional<LoadedDtb> riscv_cpus_dtb_;
  static std::optional<LoadedDtb> riscv_cpus_nested_clusters_dtb_;
  static std::optional<LoadedDtb> riscv_cpus_no_cpu_map_dtb_;
};

std::optional<LoadedDtb> RiscvDevicetreeCpuTopologyItemTest::riscv_cpus_dtb_ = std::nullopt;
std::optional<LoadedDtb> RiscvDevicetreeCpuTopologyItemTest::riscv_cpus_nested_clusters_dtb_ =
    std::nullopt;
std::optional<LoadedDtb> RiscvDevicetreeCpuTopologyItemTest::riscv_cpus_no_cpu_map_dtb_ =
    std::nullopt;

TEST_F(RiscvDevicetreeCpuTopologyItemTest, MissingNode) {
  std::array<std::byte, 1024> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = empty_fdt();
  boot_shim::DevicetreeBootShim<RiscvDevicetreeCpuTopologyItem<3>> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  ASSERT_TRUE(shim.Init());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  for (auto [header, payload] : image) {
    EXPECT_FALSE(header->type == ZBI_TYPE_CPU_TOPOLOGY);
  }
}

TEST_F(RiscvDevicetreeCpuTopologyItemTest, CpusWithCpuMap) {
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
                          .performance_class = 0xFF,
                      },
              },
          .parent_index = 0,
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
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 0,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {3, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 1,
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
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 1,
                                          .isa_strtab_index = 1,
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
                          .performance_class = 0x7F,
                      },
              },
          .parent_index = 0,
      },

      // cpu@2
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                  .processor =
                      {
                          .architecture_info =
                              {
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 2,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {2, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 4,
      },

      // cpu@3
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                  .processor =
                      {
                          .architecture_info =
                              {
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 3,
                                          .isa_strtab_index = 1,
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

  auto fdt = riscv_cpus();
  boot_shim::DevicetreeBootShim<RiscvDevicetreeCpuTopologyItem<3>> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  ASSERT_TRUE(shim.Init());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool topology_present = false;
  bool string_table_present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_CPU_TOPOLOGY) {
      topology_present = true;
      std::span<zbi_topology_node_t> nodes(reinterpret_cast<zbi_topology_node_t*>(payload.data()),
                                           payload.size() / sizeof(zbi_topology_node_t));
      boot_shim::testing::CheckCpuTopology(nodes, kExpectedTopology);
    } else if (header->type == ZBI_TYPE_RISCV64_ISA_STRTAB) {
      string_table_present = true;
      ASSERT_NO_FATAL_FAILURE(ExpectStringTable({kCommonTestHartIsaString}, payload));
    }
  }
  EXPECT_TRUE(topology_present);
  EXPECT_TRUE(string_table_present);
}

TEST_F(RiscvDevicetreeCpuTopologyItemTest, CpuNodesWithNestedClusters) {
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
                          .performance_class = 0xFF,
                      },
              },
          .parent_index = 0,
      },

      // cluster0
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster =
                      {
                          .performance_class = 0xFF,
                      },
              },
          .parent_index = 1,
      },

      // cluster0
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster =
                      {
                          .performance_class = 0xFF,
                      },
              },
          .parent_index = 2,
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
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 0,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {3, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 3,
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
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 1,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {1, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 3,
      },

      // cluster1
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
          .parent_index = 6,
      },

      // cluster2
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster =
                      {
                          .performance_class = 0x7F,
                      },
              },
          .parent_index = 7,
      },

      // cpu@2
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                  .processor =
                      {
                          .architecture_info =
                              {
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 2,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {2, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 8,
      },

      // cpu@3
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                  .processor =
                      {
                          .architecture_info =
                              {
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 3,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
                          .logical_ids = {0, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 8,
      },
  };

  std::array<std::byte, 1024> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = riscv_cpus_nested_clusters();
  boot_shim::DevicetreeBootShim<RiscvDevicetreeCpuTopologyItem<3>> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  ASSERT_TRUE(shim.Init());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool topology_present = false;
  bool string_table_present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_CPU_TOPOLOGY) {
      topology_present = true;
      std::span<zbi_topology_node_t> nodes(reinterpret_cast<zbi_topology_node_t*>(payload.data()),
                                           payload.size() / sizeof(zbi_topology_node_t));
      boot_shim::testing::CheckCpuTopology(nodes, kExpectedTopology);
    } else if (header->type == ZBI_TYPE_RISCV64_ISA_STRTAB) {
      string_table_present = true;
      ASSERT_NO_FATAL_FAILURE(ExpectStringTable({kCommonTestHartIsaString}, payload));
    }
  }
  EXPECT_TRUE(topology_present);
  EXPECT_TRUE(string_table_present);
}

TEST_F(RiscvDevicetreeCpuTopologyItemTest, CpuNodesWithoutCpuMap) {
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
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 0,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {3, 0, 0, 0},
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
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 1,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {1, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },

      // cpu@2
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                  .processor =
                      {
                          .architecture_info =
                              {
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 2,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {2, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },

      // cpu@3
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                  .processor =
                      {
                          .architecture_info =
                              {
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 3,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
                          .logical_ids = {0, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },
  };

  std::array<std::byte, 1024> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = riscv_cpus_no_cpu_map();
  boot_shim::DevicetreeBootShim<RiscvDevicetreeCpuTopologyItem<3>> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  ASSERT_TRUE(shim.Init());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool topology_present = false;
  bool string_table_present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_CPU_TOPOLOGY) {
      topology_present = true;
      std::span<zbi_topology_node_t> nodes(reinterpret_cast<zbi_topology_node_t*>(payload.data()),
                                           payload.size() / sizeof(zbi_topology_node_t));
      boot_shim::testing::CheckCpuTopology(nodes, kExpectedTopology);
    } else if (header->type == ZBI_TYPE_RISCV64_ISA_STRTAB) {
      string_table_present = true;
      ASSERT_NO_FATAL_FAILURE(ExpectStringTable({kCommonTestHartIsaString}, payload));
    }
  }
  EXPECT_TRUE(topology_present);
  EXPECT_TRUE(string_table_present);
}

TEST_F(RiscvDevicetreeCpuTopologyItemTest, Qemu) {
  constexpr std::array kExpectedTopology = {
      // cluster0
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster =
                      {
                          .performance_class = 0x1,
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
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 0,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {3, 0, 0, 0},
                          .logical_id_count = 1,
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
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 1,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {1, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },

      // cpu@2
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                  .processor =
                      {
                          .architecture_info =
                              {
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 2,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {2, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },

      // cpu@3
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                  .processor =
                      {
                          .architecture_info =
                              {
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 3,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
                          .logical_ids = {0, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },
  };

  std::array<std::byte, 1024> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = qemu_riscv();
  boot_shim::DevicetreeBootShim<RiscvDevicetreeCpuTopologyItem<3>> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  ASSERT_TRUE(shim.Init());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool topology_present = false;
  bool string_table_present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_CPU_TOPOLOGY) {
      topology_present = true;
      std::span<zbi_topology_node_t> nodes(reinterpret_cast<zbi_topology_node_t*>(payload.data()),
                                           payload.size() / sizeof(zbi_topology_node_t));
      boot_shim::testing::CheckCpuTopology(nodes, kExpectedTopology);
    } else if (header->type == ZBI_TYPE_RISCV64_ISA_STRTAB) {
      string_table_present = true;
      ASSERT_NO_FATAL_FAILURE(ExpectStringTable({kCommonTestHartIsaString}, payload));
    }
  }
  EXPECT_TRUE(topology_present);
  EXPECT_TRUE(string_table_present);
}

TEST_F(RiscvDevicetreeCpuTopologyItemTest, VisionFive2) {
  constexpr std::string_view kCommonHartIsaString = "rv64imafdc";  // strtab index 1
  constexpr std::array kExpectedTopology = {
      // cpu@0 - disabled
      //  zbi_topology_node_t{
      //      .entity =
      //          {
      //              .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
      //              .processor =
      //                  {
      //                      .architecture_info =
      //                          {
      //                              .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
      //                              .riscv64 =
      //                                  {
      //                                      .hart_id = 0,
      //                                      .isa_strtab_index = 1,
      //                                  },
      //                          },
      //                      .flags = 0,
      //                      .logical_ids = {3, 0, 0, 0},
      //                      .logical_id_count = 1,
      //                  },
      //          },
      //      .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      //  },

      // cpu@1
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                  .processor =
                      {
                          .architecture_info =
                              {
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 1,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {2, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },

      // cpu@2
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                  .processor =
                      {
                          .architecture_info =
                              {
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 2,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {1, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },

      // cpu@3
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                  .processor =
                      {
                          .architecture_info =
                              {
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 3,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
                          .logical_ids = {0, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },

      // cpu@4
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                  .processor =
                      {
                          .architecture_info =
                              {
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 4,
                                          .isa_strtab_index = 1,
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

  auto fdt = vision_five_2();
  boot_shim::DevicetreeBootShim<RiscvDevicetreeCpuTopologyItem<3>> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  ASSERT_TRUE(shim.Init());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool topology_present = false;
  bool string_table_present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_CPU_TOPOLOGY) {
      topology_present = true;
      std::span<zbi_topology_node_t> nodes(reinterpret_cast<zbi_topology_node_t*>(payload.data()),
                                           payload.size() / sizeof(zbi_topology_node_t));
      boot_shim::testing::CheckCpuTopology(nodes, kExpectedTopology);
    } else if (header->type == ZBI_TYPE_RISCV64_ISA_STRTAB) {
      string_table_present = true;
      ASSERT_NO_FATAL_FAILURE(ExpectStringTable({kCommonHartIsaString}, payload));
    }
  }
  EXPECT_TRUE(topology_present);
  EXPECT_TRUE(string_table_present);
}

TEST_F(RiscvDevicetreeCpuTopologyItemTest, HifiveSifiveUnmatched) {
  // CPU 0 is disabled.
  // constexpr std::string_view kHart0IsaString = "rv64imac";         // strtab index 1
  constexpr std::string_view kCommonHartIsaString = "rv64imafdc";  // strtab index 1
  constexpr std::array kExpectedTopology = {
      // cluster0
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_CLUSTER,
                  .cluster =
                      {
                          .performance_class = 0x1,
                      },
              },
          .parent_index = ZBI_TOPOLOGY_NO_PARENT,
      },

      // cpu@3
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                  .processor =
                      {
                          .architecture_info =
                              {
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 3,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {3, 0, 0, 0},
                          .logical_id_count = 1,
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
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 1,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {1, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },

      // cpu@4
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                  .processor =
                      {
                          .architecture_info =
                              {
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 4,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = 0,
                          .logical_ids = {2, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },

      // cpu@2
      zbi_topology_node_t{
          .entity =
              {
                  .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                  .processor =
                      {
                          .architecture_info =
                              {
                                  .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                  .riscv64 =
                                      {
                                          .hart_id = 2,
                                          .isa_strtab_index = 1,
                                      },
                              },
                          .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
                          .logical_ids = {0, 0, 0, 0},
                          .logical_id_count = 1,
                      },
              },
          .parent_index = 0,
      },

      // cpu@0 -- disabled
      // zbi_topology_node_t{
      //     .entity =
      //         {
      //             .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
      //             .processor =
      //                 {
      //                     .architecture_info =
      //                         {
      //                             .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
      //                             .riscv64 =
      //                                 {
      //                                     .hart_id = 0,
      //                                     .isa_strtab_index = 1,
      //                                 },
      //                         },
      //                     .flags = 0,
      //                     .logical_ids = {4, 0, 0, 0},
      //                     .logical_id_count = 1,
      //                 },
      //         },
      //     .parent_index = 0,
      // },
  };

  std::array<std::byte, 1024> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = sifive_hifive_unmatched();
  boot_shim::DevicetreeBootShim<RiscvDevicetreeCpuTopologyItem<2>> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  ASSERT_TRUE(shim.Init());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool topology_present = false;
  bool string_table_present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_CPU_TOPOLOGY) {
      topology_present = true;
      std::span<zbi_topology_node_t> nodes(reinterpret_cast<zbi_topology_node_t*>(payload.data()),
                                           payload.size() / sizeof(zbi_topology_node_t));
      boot_shim::testing::CheckCpuTopology(nodes, kExpectedTopology);
    } else if (header->type == ZBI_TYPE_RISCV64_ISA_STRTAB) {
      string_table_present = true;
      ASSERT_NO_FATAL_FAILURE(ExpectStringTable({kCommonHartIsaString}, payload));
    }
  }
  EXPECT_TRUE(topology_present);
  EXPECT_TRUE(string_table_present);
}

TEST_F(RiscvDevicetreeCpuTopologyItemTest, BananaPiF3) {
  constexpr std::string_view kHart0IsaString =
      "rv64imafdcv_zicsr_zifencei_zicbom_zicboz_zicbop_zihintpause_zicond_zba_zbb_zbc_zbs_svpbmt_sstc_sscofpmf_zicbom64";  // strtab index 1
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
                                  .performance_class = 0xFF,
                              },
                      },
                  .parent_index = ZBI_TOPOLOGY_NO_PARENT,
              },

              // cpu@2
              zbi_topology_node_t{
                  .entity =
                      {
                          .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                          .processor =
                              {
                                  .architecture_info =
                                      {
                                          .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                          .riscv64 =
                                              {
                                                  .hart_id = 0,
                                                  .isa_strtab_index = 1,
                                              },
                                      },
                                  .flags = 0,
                                  .logical_ids = {2, 0, 0, 0},
                                  .logical_id_count = 1,
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
                                          .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                          .riscv64 =
                                              {
                                                  .hart_id = 1,
                                                  .isa_strtab_index = 1,
                                              },
                                      },
                                  .flags = 0,
                                  .logical_ids = {1, 0, 0, 0},
                                  .logical_id_count = 1,
                              },
                      },
                  .parent_index = 0,
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
                                          .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                          .riscv64 =
                                              {
                                                  .hart_id = 2,
                                                  .isa_strtab_index = 1,
                                              },
                                      },
                                  .flags = ZBI_TOPOLOGY_PROCESSOR_FLAGS_PRIMARY,
                                  .logical_ids = {0, 0, 0, 0},
                                  .logical_id_count = 1,
                              },
                      },
                  .parent_index = 0,
              },

              // cpu@3
              zbi_topology_node_t{
                  .entity =
                      {
                          .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                          .processor =
                              {
                                  .architecture_info =
                                      {
                                          .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                          .riscv64 =
                                              {
                                                  .hart_id = 3,
                                                  .isa_strtab_index = 1,
                                              },
                                      },
                                  .flags = 0,
                                  .logical_ids = {3, 0, 0, 0},
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

              // cpu@4
              zbi_topology_node_t{
                  .entity =
                      {
                          .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                          .processor =
                              {
                                  .architecture_info =
                                      {
                                          .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                          .riscv64 =
                                              {
                                                  .hart_id = 4,
                                                  .isa_strtab_index = 1,
                                              },
                                      },
                                  .flags = 0,
                                  .logical_ids = {4, 0, 0, 0},
                                  .logical_id_count = 1,
                              },
                      },
                  .parent_index = 5,
              },

              // cpu@5
              zbi_topology_node_t{
                  .entity =
                      {
                          .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                          .processor =
                              {
                                  .architecture_info =
                                      {
                                          .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                          .riscv64 =
                                              {
                                                  .hart_id = 5,
                                                  .isa_strtab_index = 1,
                                              },
                                      },
                                  .flags = 0,
                                  .logical_ids = {5, 0, 0, 0},
                                  .logical_id_count = 1,
                              },
                      },
                  .parent_index = 5,
              },

              // cpu@6
              zbi_topology_node_t{
                  .entity =
                      {
                          .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                          .processor =
                              {
                                  .architecture_info =
                                      {
                                          .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                          .riscv64 =
                                              {
                                                  .hart_id = 6,
                                                  .isa_strtab_index = 1,
                                              },
                                      },
                                  .flags = 0,
                                  .logical_ids = {6, 0, 0, 0},
                                  .logical_id_count = 1,
                              },
                      },
                  .parent_index = 5,
              },

              // cpu@7
              zbi_topology_node_t{
                  .entity =
                      {
                          .discriminant = ZBI_TOPOLOGY_ENTITY_PROCESSOR,
                          .processor =
                              {
                                  .architecture_info =
                                      {
                                          .discriminant = ZBI_TOPOLOGY_ARCHITECTURE_INFO_RISCV64,
                                          .riscv64 =
                                              {
                                                  .hart_id = 7,
                                                  .isa_strtab_index = 1,
                                              },
                                      },
                                  .flags = 0,
                                  .logical_ids = {7, 0, 0, 0},
                                  .logical_id_count = 1,
                              },
                      },
                  .parent_index = 5,
              },
          };

  std::array<std::byte, 1024> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = banana_pi_f3();
  boot_shim::DevicetreeBootShim<RiscvDevicetreeCpuTopologyItem<2>> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  ASSERT_TRUE(shim.Init());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  bool topology_present = false;
  bool string_table_present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_CPU_TOPOLOGY) {
      topology_present = true;
      std::span<zbi_topology_node_t> nodes(reinterpret_cast<zbi_topology_node_t*>(payload.data()),
                                           payload.size() / sizeof(zbi_topology_node_t));
      boot_shim::testing::CheckCpuTopology(nodes, kExpectedTopology);
    } else if (header->type == ZBI_TYPE_RISCV64_ISA_STRTAB) {
      string_table_present = true;
      ASSERT_NO_FATAL_FAILURE(ExpectStringTable({kHart0IsaString}, payload));
    }
  }
  EXPECT_TRUE(topology_present);
  EXPECT_TRUE(string_table_present);
}

}  // namespace
