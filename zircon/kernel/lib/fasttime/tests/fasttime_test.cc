// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dlfcn.h>
#include <fcntl.h>
#include <lib/fasttime/time.h>
#include <lib/fdio/io.h>
#include <lib/fzl/owned-vmo-mapper.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <map>
#include <memory>
#include <utility>

#include <bringup/lib/restricted-machine/machine.h>
#include <bringup/lib/restricted-machine/testing/machine.h>
#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include <bringup/lib/restricted-machine/testing/fixture.gtest.h>

namespace {

constexpr char kVmoFileDir[] = "/boot/kernel";

// This fixture ensures that every test defined below is run for every supported
// restricted mode environment on the running device as well as in normal mode.
class FasttimeTest : public restricted_machine::testing::SupportedMachinesTest {
 public:
  static void SetUpTestSuite() {
    // Pull in the fasttime loadable module for use in the tests below.
    SetUpTestSuiteHelper("loadable");
  }

  void SetUp() override {
    restricted_machine::testing::SupportedMachinesTest::SetUp();
    // Create a restricted_machine::testing::Machine which handles
    // restricted mode and normal mode calls seamlessly.
    if (has_environment()) {
      machine_ = CreateMachine();
    }
  }

  void TearDown() override { machine_.reset(); }

 protected:
  std::unique_ptr<restricted_machine::Machine> machine_{};
};

TEST_P(FasttimeTest, UnaccessibleTicks) {
  zx::result<restricted_machine::Environment::Allocation> value_mem =
      environment()->Allocate(sizeof(fasttime::internal::TimeValues));
  ASSERT_TRUE(value_mem.is_ok());
  fasttime::internal::TimeValues *values =
      new (reinterpret_cast<void *>(value_mem->base)) fasttime::internal::TimeValues{
          .version = fasttime::internal::kFasttimeVersion,
          .ticks_per_second = 100000,
          .mono_ticks_offset = 12345678,
          .ticks_to_time_numerator = 125000,
          .ticks_to_time_denominator = 300000,
          .usermode_can_access_ticks = false,
          .use_a73_errata_mitigation = false,
      };

  // Test that libfasttime returns the sentinel value of ZX_TIME_INFINITE_PAST from all calls
  // when the ticks register is not usermode accessible.
  EXPECT_EQ(ZX_TIME_INFINITE_PAST,
            static_cast<long>(machine_->Call("loadable_compute_monotonic_time", values).value()));
  EXPECT_EQ(ZX_TIME_INFINITE_PAST,
            static_cast<long>(machine_->Call("loadable_compute_monotonic_ticks", values).value()));

  // Test that the unchecked variants of these calls do not return the sentinel value.
  EXPECT_NE(ZX_TIME_INFINITE_PAST,
            static_cast<long>(
                machine_->Call("loadable_compute_monotonic_time_skip_validation", values).value()));

  EXPECT_NE(
      ZX_TIME_INFINITE_PAST,
      static_cast<long>(
          machine_->Call("loadable_compute_monotonic_ticks_skip_validation", values).value()));

  // Check that the functions exposed in the fasttime namespace return the sentinel value.
  EXPECT_EQ(ZX_TIME_INFINITE_PAST,
            static_cast<long>(
                machine_->Call("loadable_fasttime_compute_monotonic_time", values).value()));
  EXPECT_EQ(ZX_TIME_INFINITE_PAST,
            static_cast<long>(
                machine_->Call("loadable_fasttime_compute_monotonic_ticks", values).value()));
}

TEST_P(FasttimeTest, MismatchedVersions) {
  auto value_mem = environment()->Allocate(sizeof(fasttime::internal::TimeValues));
  ASSERT_TRUE(value_mem.is_ok());
  fasttime::internal::TimeValues *values =
      new (reinterpret_cast<void *>(value_mem->base)) fasttime::internal::TimeValues{
          .version = fasttime::internal::kFasttimeVersion + 1,
          .ticks_per_second = 100000,
          .mono_ticks_offset = 12345678,
          .ticks_to_time_numerator = 125000,
          .ticks_to_time_denominator = 300000,
          .usermode_can_access_ticks = true,
          .use_a73_errata_mitigation = false,
      };

  // Test that a mismatch between the version of libfasttime and the version in the time_values
  // struct returns the sentinel value of ZX_TIME_INFINITE_PAST.
  EXPECT_FALSE(machine_->Call("loadable_check_fasttime_version", values).value());

  EXPECT_EQ(ZX_TIME_INFINITE_PAST,
            static_cast<long>(machine_->Call("loadable_compute_monotonic_time", values).value()));
  EXPECT_EQ(ZX_TIME_INFINITE_PAST,
            static_cast<long>(machine_->Call("loadable_compute_monotonic_ticks", values).value()));

  // Test that the unchecked variants of these calls do not return the sential value.
  EXPECT_NE(ZX_TIME_INFINITE_PAST,
            static_cast<long>(
                machine_->Call("loadable_compute_monotonic_time_skip_validation", values).value()));
  EXPECT_NE(
      ZX_TIME_INFINITE_PAST,
      static_cast<long>(
          machine_->Call("loadable_compute_monotonic_ticks_skip_validation", values).value()));

  // Check that the functions exposed in the fasttime namespace return the sentinel value.
  EXPECT_EQ(ZX_TIME_INFINITE_PAST,
            static_cast<long>(
                machine_->Call("loadable_fasttime_compute_monotonic_time", values).value()));
  EXPECT_EQ(ZX_TIME_INFINITE_PAST,
            static_cast<long>(
                machine_->Call("loadable_fasttime_compute_monotonic_ticks", values).value()));
}

TEST_P(FasttimeTest, ComputeMonotonicTicks) {
  fbl::unique_fd dir_fd(open(kVmoFileDir, O_RDONLY | O_DIRECTORY));
  fbl::unique_fd time_values_fd(openat(dir_fd.get(), kTimeValuesVmoName, O_RDONLY));
  zx::vmo time_values_vmo;
  ASSERT_EQ(fdio_get_vmo_exact(time_values_fd.get(), time_values_vmo.reset_and_get_address()),
            ZX_OK);

  uint64_t vmo_size;
  ASSERT_EQ(time_values_vmo.get_size(&vmo_size), ZX_OK);

  fzl::OwnedVmoMapper time_values_mapper;
  ASSERT_EQ(time_values_mapper.Map(std::move(time_values_vmo), 0, vmo_size, ZX_VM_PERM_READ),
            ZX_OK);
  const void *time_values_addr = time_values_mapper.start();
  // Since we cannot rely on this mapping being accessible in restricted mode
  // (kArm), we allocate space to copy the vmo as needed below.
  auto tv_copy = environment()->Allocate(vmo_size);
  ASSERT_TRUE(tv_copy.is_ok());

  // Ensure that compute_monotonic_ticks and zx_ticks_get return the same values.
  // Unfortunately, some time may elapse between these calls, so we cannot simply assert equality.
  // Instead, we invoke both functions consecutively and ascertain that the result of the first
  // call is less than that of the second.
  zx::ticks zircon_now = zx::ticks::now();
  zx::ticks fasttime_now;
  if (machine() == restricted_machine::MachineType::kArm) {
    memcpy(reinterpret_cast<void *>(tv_copy->base), time_values_addr, vmo_size);
    fasttime_now = zx::ticks(machine_
                                 ->Call("loadable_fasttime_compute_monotonic_ticks",
                                        reinterpret_cast<void *>(tv_copy->base))
                                 .value());
  } else {
    fasttime_now = zx::ticks(
        machine_->Call("loadable_fasttime_compute_monotonic_ticks", time_values_addr).value());
  }
  EXPECT_LE(zircon_now, fasttime_now);

  if (machine() == restricted_machine::MachineType::kArm) {
    memcpy(reinterpret_cast<void *>(tv_copy->base), time_values_addr, vmo_size);
    fasttime_now = zx::ticks(machine_
                                 ->Call("loadable_fasttime_compute_monotonic_ticks",
                                        reinterpret_cast<void *>(tv_copy->base))
                                 .value());
  } else {
    fasttime_now = zx::ticks(
        machine_->Call("loadable_fasttime_compute_monotonic_ticks", time_values_addr).value());
  }
  zircon_now = zx::ticks::now();
  EXPECT_LE(fasttime_now, zircon_now);
}

TEST_P(FasttimeTest, ComputeMonotonicTime) {
  fbl::unique_fd dir_fd(open(kVmoFileDir, O_RDONLY | O_DIRECTORY));
  fbl::unique_fd time_values_fd(openat(dir_fd.get(), kTimeValuesVmoName, O_RDONLY));
  zx::vmo time_values_vmo;
  ASSERT_EQ(fdio_get_vmo_exact(time_values_fd.get(), time_values_vmo.reset_and_get_address()),
            ZX_OK);

  uint64_t vmo_size;
  ASSERT_EQ(time_values_vmo.get_size(&vmo_size), ZX_OK);

  fzl::OwnedVmoMapper time_values_mapper;
  ASSERT_EQ(time_values_mapper.Map(std::move(time_values_vmo), 0, vmo_size, ZX_VM_PERM_READ),
            ZX_OK);
  const void *time_values_addr = time_values_mapper.start();
  // Since we cannot rely on this mapping being accessible in restricted mode
  // (kArm), we allocate space to copy the vmo as needed below.
  auto tv_copy = environment()->Allocate(vmo_size);
  ASSERT_TRUE(tv_copy.is_ok());

  // Ensure that compute_monotonic_time and zx_clock_get_monotonic return the same values.
  // Unfortunately, some time may elapse between these calls, so we cannot assert equality.
  // Instead, we invoke both functions consecutively and ascertain that the result of the first
  // call is less than that of the second.
  zx::time zircon_time = zx::clock::get_monotonic();
  zx::time fasttime_time;
  if (machine() == restricted_machine::MachineType::kArm) {
    memcpy(reinterpret_cast<void *>(tv_copy->base), time_values_addr, vmo_size);
    fasttime_time = zx::time(machine_
                                 ->Call("loadable_fasttime_compute_monotonic_time",
                                        reinterpret_cast<void *>(tv_copy->base))
                                 .value());
  } else {
    fasttime_time = zx::time(
        machine_->Call("loadable_fasttime_compute_monotonic_time", time_values_addr).value());
  }
  EXPECT_LE(zircon_time, fasttime_time);

  if (machine() == restricted_machine::MachineType::kArm) {
    memcpy(reinterpret_cast<void *>(tv_copy->base), time_values_addr, vmo_size);
    fasttime_time = zx::time(machine_
                                 ->Call("loadable_fasttime_compute_monotonic_time",
                                        reinterpret_cast<void *>(tv_copy->base))
                                 .value());
  } else {
    fasttime_time = zx::time(
        machine_->Call("loadable_fasttime_compute_monotonic_time", time_values_addr).value());
  }
  zircon_time = zx::clock::get_monotonic();
  EXPECT_LE(fasttime_time, zircon_time);
}

}  // namespace

// Configure the machine parameter for the test fixture.
// testing::kSupportedMachine maps kNone to normal mode in the test fixture
// and test machine used above.
INSTANTIATE_TEST_SUITE_P(/* no prefix */, FasttimeTest,
                         testing::ValuesIn(::restricted_machine::testing::kSupportedMachines),
                         ::restricted_machine::testing::SupportedMachinesTest::ParamToText);
