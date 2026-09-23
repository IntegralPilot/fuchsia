// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree-boot-shim.h>
#include <lib/boot-shim/devicetree.h>
#include <lib/boot-shim/testing/devicetree-test-fixture.h>
#include <lib/fit/defer.h>
#include <lib/zbitl/image.h>

namespace {

using boot_shim::testing::LoadDtb;
using boot_shim::testing::LoadedDtb;

class TestAllocator {
 public:
  TestAllocator() = default;
  TestAllocator(TestAllocator&& other) {
    allocs_ = std::move(other.allocs_);
    other.allocs_.clear();
  }

  ~TestAllocator() {
    for (uint8_t* alloc : allocs_) {
      delete[] alloc;
    }
  }

  void* operator()(size_t size, size_t alignment, fbl::AllocChecker& ac) {
    uint8_t* alloc = new (static_cast<std::align_val_t>(alignment), &ac) uint8_t[size];
    allocs_.push_back(alloc);
    return reinterpret_cast<void*>(alloc);
  }

 private:
  std::vector<uint8_t*> allocs_;
};

class ArmDevicetreePsciCpuSuspendItemTest
    : public boot_shim::testing::TestMixin<boot_shim::testing::ArmDevicetreeTest,
                                           boot_shim::testing::SyntheticDevicetreeTest> {
 public:
  static void SetUpTestSuite() {
    Mixin::SetUpTestSuite();
    auto loaded_dtb = LoadDtb("arm_idle_states.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    idle_states_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("arm_idle_states_and_domain.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    idle_states_and_domain_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("arm_idle_states_multiple.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    idle_states_multiple_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("arm_idle_states_multiple_with_invalid.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    idle_states_multiple_with_invalid_ = std::move(loaded_dtb).value();
  }

  static void TearDownTestSuite() {
    idle_states_ = std::nullopt;
    idle_states_and_domain_ = std::nullopt;
    idle_states_multiple_ = std::nullopt;
    idle_states_multiple_with_invalid_ = std::nullopt;
    Mixin::TearDownTestSuite();
  }

  devicetree::Devicetree idle_states() { return idle_states_->fdt(); }
  devicetree::Devicetree idle_states_and_domain() { return idle_states_and_domain_->fdt(); }
  devicetree::Devicetree idle_states_multiple() { return idle_states_multiple_->fdt(); }
  devicetree::Devicetree idle_states_multiple_with_invalid() {
    return idle_states_multiple_with_invalid_->fdt();
  }

 private:
  static inline std::optional<LoadedDtb> idle_states_;
  static inline std::optional<LoadedDtb> idle_states_and_domain_;
  static inline std::optional<LoadedDtb> idle_states_multiple_;
  static inline std::optional<LoadedDtb> idle_states_multiple_with_invalid_;
};

TEST_F(ArmDevicetreePsciCpuSuspendItemTest, MissingNode) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = empty_fdt();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreePsciCpuSuspendItem> shim("test", fdt);
  shim.set_allocator(TestAllocator());
  ASSERT_TRUE(shim.Init());

  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  for (auto [header, payload] : image) {
    EXPECT_FALSE(header->type == ZBI_TYPE_KERNEL_DRIVER &&
                 header->extra == ZBI_KERNEL_DRIVER_ARM_PSCI_CPU_SUSPEND);
  }
}

struct ExpectedIdleState {
  uint32_t psci_param;
  uint32_t entry;
  uint32_t exit;
  uint32_t residency;
  bool is_domain_idle_state;
  bool local_timer_stops;
};

void CheckIdleStates(zbitl::Image<std::span<std::byte>>& zbi,
                     std::span<const ExpectedIdleState> states) {
  bool present = false;

  auto clear_errors = fit::defer([&]() { zbi.ignore_error(); });
  for (const auto& [header, payload] : zbi) {
    if (header->type != ZBI_TYPE_KERNEL_DRIVER) {
      continue;
    }
    if (header->extra != ZBI_KERNEL_DRIVER_ARM_PSCI_CPU_SUSPEND) {
      continue;
    }
    present = true;
    ASSERT_EQ(header->length, sizeof(zbi_dcfg_arm_psci_cpu_suspend_state_t) * states.size());
    auto* psci_states = reinterpret_cast<zbi_dcfg_arm_psci_cpu_suspend_state_t*>(payload.data());
    // Simple lookup based on the psci parameter.
    for (const auto& state : states) {
      bool state_found = false;
      for (const auto& psci_state : std::span(psci_states, states.size())) {
        if (psci_state.power_state != state.psci_param) {
          continue;
        }
        state_found = true;
        EXPECT_EQ(
            (psci_state.flags & ZBI_ARM_PSCI_CPU_SUSPEND_STATE_FLAGS_TARGETS_POWER_DOMAIN) != 0,
            state.is_domain_idle_state);
        EXPECT_EQ((psci_state.flags & ZBI_ARM_PSCI_CPU_SUSPEND_STATE_FLAGS_LOCAL_TIMER_STOPS) != 0,
                  state.local_timer_stops);
        EXPECT_EQ(psci_state.entry_latency_us, state.entry);
        EXPECT_EQ(psci_state.exit_latency_us, state.exit);
        EXPECT_EQ(psci_state.min_residency_us, state.residency);
      }
      ASSERT_TRUE(state_found);
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(ArmDevicetreePsciCpuSuspendItemTest, IdleStates) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = idle_states();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreePsciCpuSuspendItem> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  constexpr auto kExpected = std::to_array<ExpectedIdleState>({
      {
          .psci_param = 0x40000003,
          .entry = 0x123,
          .exit = 0x456,
          .residency = 0x589,
          .is_domain_idle_state = false,
          .local_timer_stops = true,
      },
      {
          .psci_param = 0x41000043,
          .entry = 0x321,
          .exit = 0x456,
          .residency = 0x987,
          .is_domain_idle_state = true,
          .local_timer_stops = false,
      },
  });

  CheckIdleStates(image, kExpected);
}

TEST_F(ArmDevicetreePsciCpuSuspendItemTest, IdleStatesAndDomainStates) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = idle_states_and_domain();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreePsciCpuSuspendItem> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  constexpr auto kExpected = std::to_array<ExpectedIdleState>({
      {
          .psci_param = 0x40000003,
          .entry = 0x123,
          .exit = 0x456,
          .residency = 0x589,
          .is_domain_idle_state = false,
          .local_timer_stops = true,
      },
      {
          .psci_param = 0x41000043,
          .entry = 0x321,
          .exit = 0x456,
          .residency = 0x987,
          .is_domain_idle_state = true,
          .local_timer_stops = false,
      },
  });

  CheckIdleStates(image, kExpected);
}

TEST_F(ArmDevicetreePsciCpuSuspendItemTest, IdleStatesMultiple) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = idle_states_multiple();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreePsciCpuSuspendItem> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  constexpr auto kExpected = std::to_array<ExpectedIdleState>({
      {
          .psci_param = 0x40000003,
          .entry = 0x123,
          .exit = 0x456,
          .residency = 0x589,
          .is_domain_idle_state = false,
          .local_timer_stops = true,
      },
      {
          .psci_param = 0x40000004,
          .entry = 0x124,
          .exit = 0x457,
          .residency = 0x580,
          .is_domain_idle_state = false,
          .local_timer_stops = false,
      },
      {
          .psci_param = 0x40000005,
          .entry = 0x122,
          .exit = 0x455,
          .residency = 0x588,
          .is_domain_idle_state = false,
          .local_timer_stops = true,
      },
      {
          .psci_param = 0x41000043,
          .entry = 0x321,
          .exit = 0x456,
          .residency = 0x987,
          .is_domain_idle_state = true,
          .local_timer_stops = false,
      },
  });

  CheckIdleStates(image, kExpected);
}

TEST_F(ArmDevicetreePsciCpuSuspendItemTest, IdleStatesMultipleWithInvalid) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = idle_states_multiple_with_invalid();
  boot_shim::DevicetreeBootShim<boot_shim::ArmDevicetreePsciCpuSuspendItem> shim("test", fdt);
  shim.set_allocator(TestAllocator());

  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  constexpr auto kExpected = std::to_array<ExpectedIdleState>({
      {
          .psci_param = 0x40000003,
          .entry = 0x123,
          .exit = 0x456,
          .residency = 0x589,
          .is_domain_idle_state = false,
          .local_timer_stops = true,
      },
      {
          .psci_param = 0x41000043,
          .entry = 0x321,
          .exit = 0x456,
          .residency = 0x987,
          .is_domain_idle_state = true,
          .local_timer_stops = false,
      },
  });

  CheckIdleStates(image, kExpected);
}

}  // namespace
