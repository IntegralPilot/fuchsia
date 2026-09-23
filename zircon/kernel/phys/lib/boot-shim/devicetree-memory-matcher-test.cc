// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree-boot-shim.h>
#include <lib/boot-shim/devicetree.h>
#include <lib/boot-shim/testing/devicetree-test-fixture.h>
#include <lib/zbitl/image.h>

#include <optional>

namespace {
using boot_shim::testing::LoadDtb;
using boot_shim::testing::LoadedDtb;
using devicetree::MemoryReservation;

class MemoryMatcherTest
    : public boot_shim::testing::TestMixin<boot_shim::testing::SyntheticDevicetreeTest,
                                           boot_shim::testing::RiscvDevicetreeTest,
                                           boot_shim::testing::ArmDevicetreeTest> {
 public:
  static void SetUpTestSuite() {
    Mixin::SetUpTestSuite();
    auto loaded_dtb = LoadDtb("memory.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    memory_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("reserved_memory.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    reserved_memory_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("memory_reservations.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    memreserve_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("memory_complex.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    complex_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("ramoops.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    ramoops_ = std::move(loaded_dtb).value();
  }

  static void TearDownTestSuite() {
    memory_ = std::nullopt;
    reserved_memory_ = std::nullopt;
    memreserve_ = std::nullopt;
    complex_ = std::nullopt;
    ramoops_ = std::nullopt;
    Mixin::TearDownTestSuite();
  }

  devicetree::Devicetree memory() { return memory_->fdt(); }
  devicetree::Devicetree reserved_memory() { return reserved_memory_->fdt(); }
  devicetree::Devicetree memreserve() { return memreserve_->fdt(); }
  devicetree::Devicetree complex() { return complex_->fdt(); }
  devicetree::Devicetree ramoops() { return ramoops_->fdt(); }

 private:
  static std::optional<LoadedDtb> memory_;
  static std::optional<LoadedDtb> reserved_memory_;
  static std::optional<LoadedDtb> memreserve_;
  static std::optional<LoadedDtb> complex_;
  static std::optional<LoadedDtb> ramoops_;
};

std::optional<LoadedDtb> MemoryMatcherTest::memory_ = std::nullopt;
std::optional<LoadedDtb> MemoryMatcherTest::reserved_memory_ = std::nullopt;
std::optional<LoadedDtb> MemoryMatcherTest::memreserve_ = std::nullopt;
std::optional<LoadedDtb> MemoryMatcherTest::complex_ = std::nullopt;
std::optional<LoadedDtb> MemoryMatcherTest::ramoops_ = std::nullopt;

TEST_F(MemoryMatcherTest, ParseMemreserves) {
  std::vector<MemoryReservation> reservations;

  auto fdt = memreserve();
  ASSERT_TRUE(boot_shim::ForEachDevicetreeMemoryReservation(fdt, {},
                                                            [&reservations](MemoryReservation res) {
                                                              reservations.push_back(res);
                                                              return true;
                                                            }));

  ASSERT_EQ(reservations.size(), 4);

  // Sort in lexicographic order for stable comparison.
  std::sort(reservations.begin(), reservations.end(), [](auto a, auto b) {
    return a.start < b.start || (a.start == b.start && a.size < b.size);
  });

  // [0x12340000, 0x12342000)
  EXPECT_EQ(reservations[0].start, 0x12340000);
  EXPECT_EQ(reservations[0].size, 0x2000);

  // [0x56780000, 0x56783000)
  EXPECT_EQ(reservations[1].start, 0x56780000);
  EXPECT_EQ(reservations[1].size, 0x3000);

  // [0x00ffffff56780000, 0x00ffffffa6780000)
  EXPECT_EQ(reservations[2].start, 0x00ffffff56780000);
  EXPECT_EQ(reservations[2].size, 0x500000000);

  // [0x7fffffff12340000, 0x8000000312340000)
  EXPECT_EQ(reservations[3].start, 0x7fffffff12340000);
  EXPECT_EQ(reservations[3].size, 0x400000000);
}

TEST_F(MemoryMatcherTest, ReservationsWithNonOverlappingExclusions) {
  std::vector<MemoryReservation> reservations;

  constexpr std::array kExclusions = {
      // [0, 0x12340000): Ends right a first reservation.
      memalloc::Range{.addr = 0, .size = 0x12340000, .type = memalloc::Type::kPoolTestPayload},

      // [0x12342000, 0x56780000): In between the first two reservations.
      memalloc::Range{
          .addr = 0x12342000, .size = 0x4443e000, .type = memalloc::Type::kPoolTestPayload},
  };

  auto fdt = memreserve();
  ASSERT_TRUE(boot_shim::ForEachDevicetreeMemoryReservation(fdt, {kExclusions},
                                                            [&reservations](MemoryReservation res) {
                                                              reservations.push_back(res);
                                                              return true;
                                                            }));

  ASSERT_EQ(reservations.size(), 4);

  // Sort in lexicographic order for stable comparison.
  std::sort(reservations.begin(), reservations.end(), [](auto a, auto b) {
    return a.start < b.start || (a.start == b.start && a.size < b.size);
  });

  // [0x12340000, 0x12342000)
  EXPECT_EQ(reservations[0].start, 0x12340000);
  EXPECT_EQ(reservations[0].size, 0x2000);

  // [0x56780000, 0x56783000)
  EXPECT_EQ(reservations[1].start, 0x56780000);
  EXPECT_EQ(reservations[1].size, 0x3000);

  // [0x00ffffff56780000, 0x00ffffffa6780000)
  EXPECT_EQ(reservations[2].start, 0x00ffffff56780000);
  EXPECT_EQ(reservations[2].size, 0x500000000);

  // [0x7fffffff12340000, 0x8000000312340000)
  EXPECT_EQ(reservations[3].start, 0x7fffffff12340000);
  EXPECT_EQ(reservations[3].size, 0x400000000);
}

TEST_F(MemoryMatcherTest, ReservationsWithOverlappingExclusions) {
  std::vector<MemoryReservation> reservations;

  constexpr std::array kExclusions = {
      // [0x1233f000, 0x12341000): Overlaps with the head of the first reservation.
      memalloc::Range{.addr = 0x1233f000, .size = 0x2000, .type = memalloc::Type::kPoolTestPayload},

      // [0x56781000, 0x56784000): Overlaps with the tail of the second reservation.
      memalloc::Range{.addr = 0x56781000, .size = 0x3000, .type = memalloc::Type::kPoolTestPayload},

      // [0x00ffffff56790000, 0x00ffffff567a0000): Contained within the third reservation.
      memalloc::Range{
          .addr = 0x00ffffff56790000, .size = 0x10000, .type = memalloc::Type::kPoolTestPayload},

      // [0x7fffffff12340000, 0x8000000312340000): Spans the fourth reservation.
      memalloc::Range{.addr = 0x7fffffff12340000,
                      .size = 0x400000000,
                      .type = memalloc::Type::kPoolTestPayload},
  };

  auto fdt = memreserve();
  ASSERT_TRUE(boot_shim::ForEachDevicetreeMemoryReservation(fdt, {kExclusions},
                                                            [&reservations](MemoryReservation res) {
                                                              reservations.push_back(res);
                                                              return true;
                                                            }));

  ASSERT_EQ(reservations.size(), 4);

  // Sort in lexicographic order for stable comparison.
  std::sort(reservations.begin(), reservations.end(), [](auto a, auto b) {
    return a.start < b.start || (a.start == b.start && a.size < b.size);
  });

  // [0x12341000, 0x12342000) ( originally from [0x12340000, 0x12342000) )
  EXPECT_EQ(reservations[0].start, 0x12341000);
  EXPECT_EQ(reservations[0].size, 0x1000);

  // [0x56780000, 0x56781000) ( originally from [0x56780000, 0x56783000) )
  EXPECT_EQ(reservations[1].start, 0x56780000);
  EXPECT_EQ(reservations[1].size, 0x1000);

  // [0x00ffffff56780000, 0x00ffffff56790000) ( originally from
  // [0x00ffffff56780000, 0x00ffffffa6780000) )
  EXPECT_EQ(reservations[2].start, 0x00ffffff56780000);
  EXPECT_EQ(reservations[2].size, 0x10000);

  // [0x00ffffff567a0000, 0x00ffffffa6780000) ( originally from
  // [0x00ffffff56780000, 0x00ffffffa6780000) )
  EXPECT_EQ(reservations[3].start, 0x00ffffff567a0000);
  EXPECT_EQ(reservations[3].size, 0x4fffe0000);
}

TEST_F(MemoryMatcherTest, ParseMemoryNodes) {
  std::vector<memalloc::Range> storage(5);

  auto fdt = memory();
  boot_shim::DevicetreeMemoryMatcher memory_matcher("test", stdout, storage);

  ASSERT_TRUE(devicetree::Match(fdt, memory_matcher));
  auto ranges = memory_matcher.ranges();
  ASSERT_EQ(ranges.size(), 4);

  // Each memory nodes in order.
  EXPECT_EQ(ranges[0].addr, 0x40000000);
  EXPECT_EQ(ranges[0].size, 0x10000000);
  EXPECT_EQ(ranges[0].type, memalloc::Type::kFreeRam);

  EXPECT_EQ(ranges[1].addr, 0x50000000);
  EXPECT_EQ(ranges[1].size, 0x20000000);
  EXPECT_EQ(ranges[1].type, memalloc::Type::kFreeRam);

  EXPECT_EQ(ranges[2].addr, 0x60000000);
  EXPECT_EQ(ranges[2].size, 0x30000000);
  EXPECT_EQ(ranges[2].type, memalloc::Type::kFreeRam);

  EXPECT_EQ(ranges[3].addr, 0x70000000);
  EXPECT_EQ(ranges[3].size, 0x40000000);
  EXPECT_EQ(ranges[3].type, memalloc::Type::kFreeRam);
}

TEST_F(MemoryMatcherTest, ParseReservedMemoryNodes) {
  std::vector<memalloc::Range> storage(3);

  auto fdt = reserved_memory();
  boot_shim::DevicetreeMemoryMatcher memory_matcher("test", stdout, storage);

  ASSERT_TRUE(devicetree::Match(fdt, memory_matcher));
  auto ranges = memory_matcher.ranges();
  ASSERT_EQ(ranges.size(), 2);

  // Each reserved memory nodes in order.
  EXPECT_EQ(ranges[0].addr, 0x78000000);
  EXPECT_EQ(ranges[0].size, 0x800000);
  EXPECT_EQ(ranges[0].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[1].addr, 0x76000000);
  EXPECT_EQ(ranges[1].size, 0x400000);
  EXPECT_EQ(ranges[1].type, memalloc::Type::kReserved);
}

TEST_F(MemoryMatcherTest, ParseAll) {
  std::vector<memalloc::Range> storage(11);

  auto fdt = complex();
  boot_shim::DevicetreeMemoryMatcher memory_matcher("test", stdout, storage);

  ASSERT_TRUE(devicetree::Match(fdt, memory_matcher));
  auto ranges = memory_matcher.ranges();
  ASSERT_EQ(ranges.size(), 6);

  // Each memory nodes in order.
  EXPECT_EQ(ranges[0].addr, 0x40000000);
  EXPECT_EQ(ranges[0].size, 0x10000000);
  EXPECT_EQ(ranges[0].type, memalloc::Type::kFreeRam);

  EXPECT_EQ(ranges[1].addr, 0x50000000);
  EXPECT_EQ(ranges[1].size, 0x20000000);
  EXPECT_EQ(ranges[1].type, memalloc::Type::kFreeRam);

  EXPECT_EQ(ranges[2].addr, 0x60000000);
  EXPECT_EQ(ranges[2].size, 0x30000000);
  EXPECT_EQ(ranges[3].type, memalloc::Type::kFreeRam);

  EXPECT_EQ(ranges[3].addr, 0x70000000);
  EXPECT_EQ(ranges[3].size, 0x40000000);
  EXPECT_EQ(ranges[3].type, memalloc::Type::kFreeRam);

  // Each reserved memory nodes in order.
  EXPECT_EQ(ranges[4].addr, 0x78000000);
  EXPECT_EQ(ranges[4].size, 0x800000);
  EXPECT_EQ(ranges[4].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[5].addr, 0x76000000);
  EXPECT_EQ(ranges[5].size, 0x400000);
  EXPECT_EQ(ranges[5].type, memalloc::Type::kReserved);
}

TEST_F(MemoryMatcherTest, QemuRiscv) {
  std::vector<memalloc::Range> storage(3);

  auto fdt = qemu_riscv();
  boot_shim::DevicetreeMemoryMatcher memory_matcher("test", stdout, storage);

  ASSERT_TRUE(devicetree::Match(fdt, memory_matcher));
  auto ranges = memory_matcher.ranges();
  ASSERT_EQ(ranges.size(), 2);

  // Account for the devicetree in use.
  EXPECT_EQ(ranges[0].addr, 0x80000000);
  EXPECT_EQ(ranges[0].size, 0x80000);
  EXPECT_EQ(ranges[0].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[1].addr, 0x80000000);
  EXPECT_EQ(ranges[1].size, 0x100000000);
  EXPECT_EQ(ranges[1].type, memalloc::Type::kFreeRam);
}

TEST_F(MemoryMatcherTest, VisionFive2) {
  std::vector<memalloc::Range> storage(9);

  auto fdt = vision_five_2();
  boot_shim::DevicetreeMemoryMatcher memory_matcher("test", stdout, storage);

  ASSERT_TRUE(devicetree::Match(fdt, memory_matcher));
  auto ranges = memory_matcher.ranges();
  ASSERT_EQ(ranges.size(), 7);

  EXPECT_EQ(ranges[0].addr, 0x40000000);
  EXPECT_EQ(ranges[0].size, 0x200000000);
  EXPECT_EQ(ranges[0].type, memalloc::Type::kFreeRam);

  EXPECT_EQ(ranges[1].addr, 0x40000000);
  EXPECT_EQ(ranges[1].size, 0x80000);
  EXPECT_EQ(ranges[1].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[2].addr, 0xc0110000);
  EXPECT_EQ(ranges[2].size, 0xf0000);
  EXPECT_EQ(ranges[2].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[3].addr, 0xf0000000);
  EXPECT_EQ(ranges[3].size, 0x1ffffff);
  EXPECT_EQ(ranges[3].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[4].addr, 0xf2000000);
  EXPECT_EQ(ranges[4].size, 0x1000);
  EXPECT_EQ(ranges[4].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[5].addr, 0xf2001000);
  EXPECT_EQ(ranges[5].size, 0xfff000);
  EXPECT_EQ(ranges[5].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[6].addr, 0xf3000000);
  EXPECT_EQ(ranges[6].size, 0x1000);
  EXPECT_EQ(ranges[6].type, memalloc::Type::kReserved);
}

TEST_F(MemoryMatcherTest, SifiveHifiveUnmatched) {
  std::vector<memalloc::Range> storage(3);

  auto fdt = sifive_hifive_unmatched();
  boot_shim::DevicetreeMemoryMatcher memory_matcher("test", stdout, storage);

  ASSERT_TRUE(devicetree::Match(fdt, memory_matcher));
  auto ranges = memory_matcher.ranges();
  ASSERT_EQ(ranges.size(), 2);

  EXPECT_EQ(ranges[0].addr, 0x80000000);
  EXPECT_EQ(ranges[0].size, 0x400000000);
  EXPECT_EQ(ranges[0].type, memalloc::Type::kFreeRam);

  EXPECT_EQ(ranges[1].addr, 0x80000000);
  EXPECT_EQ(ranges[1].size, 0x80000);
  EXPECT_EQ(ranges[1].type, memalloc::Type::kReserved);
}

TEST_F(MemoryMatcherTest, QemuArm) {
  std::vector<memalloc::Range> storage(2);

  auto fdt = qemu_arm_gic3();
  boot_shim::DevicetreeMemoryMatcher memory_matcher("test", stdout, storage);

  ASSERT_TRUE(devicetree::Match(fdt, memory_matcher));
  auto ranges = memory_matcher.ranges();
  ASSERT_EQ(ranges.size(), 1);

  EXPECT_EQ(ranges[0].addr, 0x40000000);
  EXPECT_EQ(ranges[0].size, 0x200000000);
  EXPECT_EQ(ranges[0].type, memalloc::Type::kFreeRam);
}

TEST_F(MemoryMatcherTest, CrosvmArm) {
  std::vector<memalloc::Range> storage(2);

  auto fdt = crosvm_arm();
  boot_shim::DevicetreeMemoryMatcher memory_matcher("test", stdout, storage);

  ASSERT_TRUE(devicetree::Match(fdt, memory_matcher));
  auto ranges = memory_matcher.ranges();
  ASSERT_EQ(ranges.size(), 1);

  EXPECT_EQ(ranges[0].addr, 0x80000000);
  EXPECT_EQ(ranges[0].size, 0x25800000);
  EXPECT_EQ(ranges[0].type, memalloc::Type::kFreeRam);
}

TEST_F(MemoryMatcherTest, KhadasVim3) {
  std::vector<memalloc::Range> storage(5);

  auto fdt = khadas_vim3();
  boot_shim::DevicetreeMemoryMatcher memory_matcher("test", stdout, storage);

  ASSERT_TRUE(devicetree::Match(fdt, memory_matcher));
  auto ranges = memory_matcher.ranges();
  ASSERT_EQ(ranges.size(), 3);

  EXPECT_EQ(ranges[0].addr, 0x00);
  EXPECT_EQ(ranges[0].size, 0xf4e5b000);
  EXPECT_EQ(ranges[0].type, memalloc::Type::kFreeRam);

  EXPECT_EQ(ranges[1].addr, 0x5000000);
  EXPECT_EQ(ranges[1].size, 0x300000);
  EXPECT_EQ(ranges[1].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[2].addr, 0x5300000);
  EXPECT_EQ(ranges[2].size, 0x2000000);
  EXPECT_EQ(ranges[2].type, memalloc::Type::kReserved);

  ASSERT_TRUE(memory_matcher.nvram());
  EXPECT_EQ(memory_matcher.nvram()->base, 0xd000000);
  EXPECT_EQ(memory_matcher.nvram()->length, 0x100000);
}

TEST_F(MemoryMatcherTest, BananaPiF3) {
  std::vector<memalloc::Range> storage(13);

  auto fdt = banana_pi_f3();
  boot_shim::DevicetreeMemoryMatcher memory_matcher("test", stdout, storage);

  ASSERT_TRUE(devicetree::Match(fdt, memory_matcher));
  auto ranges = memory_matcher.ranges();
  ASSERT_EQ(ranges.size(), 13);

  EXPECT_EQ(ranges[0].addr, 0x00);
  EXPECT_EQ(ranges[0].size, 0x80000000);
  EXPECT_EQ(ranges[0].type, memalloc::Type::kFreeRam);

  EXPECT_EQ(ranges[1].addr, 0x100000000);
  EXPECT_EQ(ranges[1].size, 0x80000000);
  EXPECT_EQ(ranges[1].type, memalloc::Type::kFreeRam);

  EXPECT_EQ(ranges[2].addr, 0x0);
  EXPECT_EQ(ranges[2].size, 0x80000);
  EXPECT_EQ(ranges[2].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[3].addr, 0x80000000);
  EXPECT_EQ(ranges[3].size, 0x40800000);
  EXPECT_EQ(ranges[3].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[4].addr, 0xc0800000);
  EXPECT_EQ(ranges[4].size, 0x40000);
  EXPECT_EQ(ranges[4].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[5].addr, 0xc0840000);
  EXPECT_EQ(ranges[5].size, 0x3f7c0000);
  EXPECT_EQ(ranges[5].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[6].addr, 0x30000000);
  EXPECT_EQ(ranges[6].size, 0x200000);
  EXPECT_EQ(ranges[6].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[7].addr, 0x30200000);
  EXPECT_EQ(ranges[7].size, 0x3000);
  EXPECT_EQ(ranges[7].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[8].addr, 0x30203000);
  EXPECT_EQ(ranges[8].size, 0x3000);
  EXPECT_EQ(ranges[8].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[9].addr, 0x30206000);
  EXPECT_EQ(ranges[9].size, 0xf6000);
  EXPECT_EQ(ranges[9].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[10].addr, 0x302fc000);
  EXPECT_EQ(ranges[10].size, 0x4000);
  EXPECT_EQ(ranges[10].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[11].addr, 0x30300000);
  EXPECT_EQ(ranges[11].size, 0x40000);
  EXPECT_EQ(ranges[11].type, memalloc::Type::kReserved);

  EXPECT_EQ(ranges[12].addr, 0x2ff40000);
  EXPECT_EQ(ranges[12].size, 0xc0000);
  EXPECT_EQ(ranges[12].type, memalloc::Type::kReserved);
}

TEST_F(MemoryMatcherTest, NoNvramGeneratesNoItemWhenMissing) {
  std::vector<memalloc::Range> storage(13);
  boot_shim::DevicetreeMemoryMatcher matcher("test", stdout, storage);
  auto fdt = empty_fdt();
  ASSERT_TRUE(devicetree::Match(fdt, matcher));
  ASSERT_FALSE(matcher.nvram());
}

TEST_F(MemoryMatcherTest, HasNvramGeneratesItemWhenPresent) {
  std::vector<memalloc::Range> storage(13);

  boot_shim::DevicetreeMemoryMatcher matcher("test", stdout, storage);
  auto fdt = ramoops();
  ASSERT_TRUE(devicetree::Match(fdt, matcher));
  ASSERT_TRUE(matcher.nvram());

  EXPECT_EQ(matcher.nvram()->base, 0x8f000000);
  EXPECT_EQ(matcher.nvram()->length, 0x100000);
}

}  // namespace
