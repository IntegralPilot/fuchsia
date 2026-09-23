// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree-boot-shim.h>
#include <lib/boot-shim/devicetree.h>
#include <lib/boot-shim/item-base.h>
#include <lib/boot-shim/uart.h>
#include <lib/devicetree/devicetree.h>
#include <lib/devicetree/matcher.h>
#include <lib/devicetree/testing/loaded-dtb.h>
#include <lib/fit/function.h>
#include <lib/zbitl/image.h>

#include <array>
#include <cstdint>
#include <iostream>
#include <span>
#include <string>
#include <string_view>

#include <fbl/alloc_checker.h>
#include <zxtest/zxtest.h>

namespace {

struct ArmZeroMpidr {
  static arch::ArmMultiprocessorAffinityRegister Read() {
    arch::ArmMultiprocessorAffinityRegister mpidr;
    mpidr.set_reg_value(0);
    return mpidr;
  }
};

class FakeMatcher : public boot_shim::DevicetreeItemBase<FakeMatcher, 2> {
 public:
  template <typename T>
  FakeMatcher(std::string_view target, std::string_view cmdline_name, int count, T&& set_payload)
      : target_(target),
        cmdline_name_(cmdline_name),
        max_count_(count),
        set_payload_(set_payload) {}

  devicetree::ScanState OnNode(const devicetree::NodePath& path,
                               const devicetree::PropertyDecoder& decoder) {
    auto resolved_path = decoder.ResolvePath(target_);
    if (resolved_path.is_error()) {
      return resolved_path.error_value() == devicetree::PropertyDecoder::PathResolveError::kBadAlias
                 ? devicetree::ScanState::kDoneWithSubtree
                 : devicetree::ScanState::kNeedsPathResolution;
    }
    switch (path.CompareWith(*resolved_path)) {
      case devicetree::NodePath::Comparison::kParent:
      case devicetree::NodePath::Comparison::kIndirectAncestor:
        return devicetree::ScanState::kActive;

      case devicetree::NodePath::Comparison::kEqual: {
        count_++;
        if (count_ == max_count_) {
          return devicetree::ScanState::kDone;
        }
        return devicetree::ScanState::kDoneWithSubtree;
      }

      case devicetree::NodePath::Comparison::kChild:
      case devicetree::NodePath::Comparison::kIndirectDescendent:
      case devicetree::NodePath::Comparison::kMismatch:
        return devicetree::ScanState::kDoneWithSubtree;
    }
  }

  devicetree::ScanState OnSubtree(const devicetree::NodePath& path) {
    return devicetree::ScanState::kActive;
  }

  devicetree::ScanState OnScan() {
    return value_.empty() ? devicetree::ScanState::kActive : devicetree::ScanState::kDone;
  }

  void OnDone() {
    value_ = cmdline_name_ + std::to_string(count_);
    set_payload_(value_);
  }

  void OnError(std::string_view err) { std::cout << " Matcher error " << err << std::endl; }

 private:
  std::string_view target_;
  std::string cmdline_name_;
  std::string value_;
  int count_ = 0;
  int max_count_ = 0;
  fit::function<void(std::string_view)> set_payload_;
};

class DevicetreeItem1
    : public FakeMatcher,
      public boot_shim::SingleOptionalItem<std::array<char, 30>, ZBI_TYPE_CMDLINE> {
 public:
  DevicetreeItem1()
      : FakeMatcher("bar/G/H", "--visit-count=", 1, [this](std::string_view payload) {
          std::array<char, 30> msg = {};
          memcpy(msg.data(), payload.data(), payload.size());
          this->set_payload(msg);
        }) {}

 private:
  std::string cmdline_;
};
static_assert(devicetree::Matcher<DevicetreeItem1>);

class DevicetreeItem2
    : public FakeMatcher,
      public boot_shim::SingleOptionalItem<std::array<char, 30>, ZBI_TYPE_CMDLINE> {
 public:
  DevicetreeItem2()
      : FakeMatcher("/E/F/G/H", "--visit-count-b=", 2, [this](std::string_view payload) {
          std::array<char, 30> msg = {};
          memcpy(msg.data(), payload.data(), payload.size());
          this->set_payload(msg);
        }) {}

  template <typename T>
  void Init(const T& shim) {}
};

using NonDeviceTreeItem = boot_shim::SingleItem<1>;
using devicetree::testing::LoadDtb;
using devicetree::testing::LoadedDtb;

class DevicetreeBootShimTest : public zxtest::Test {
 public:
  static void SetUpTestSuite() {
    if (!ldtb_) {
      auto loaded_dtb = LoadDtb("complex_with_alias_first.dtb");
      ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
      ldtb_ = std::move(loaded_dtb).value();
    }
  }
  /*
  *
     /     / \
   //aliases A   E
     / \   \
         //B   C   F
     /   / \
           //D   G   I
  /
   H
   aliases:
   foo = /A/C
   bar = /E/F
  */
  devicetree::Devicetree fdt() { return ldtb_->fdt(); }

 private:
  static std::optional<LoadedDtb> ldtb_;
};

std::optional<LoadedDtb> DevicetreeBootShimTest::ldtb_;

void CheckZbiHasItemWithContent(zbitl::Image<std::span<std::byte>> image, uint32_t item_type,
                                std::string_view contents) {
  int count = 0;
  for (auto it : image) {
    auto [h, p] = it;
    EXPECT_EQ(h->type, ZBI_TYPE_CMDLINE);
    EXPECT_EQ(h->extra, 0);
    std::string_view s(reinterpret_cast<const char*>(p.data()), p.size());
    if (s.find(contents) == 0) {
      count++;
    }
  }
  image.ignore_error();
  EXPECT_EQ(count, 1);
}

TEST_F(DevicetreeBootShimTest, DevicetreeItemWithAlias) {
  std::array<std::byte, 256> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());
  boot_shim::DevicetreeBootShim<DevicetreeItem1> shim("devicetree-boot-shim-test", fdt());
  ASSERT_TRUE(shim.Init());

  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  CheckZbiHasItemWithContent(image, ZBI_TYPE_CMDLINE, "--visit-count=1");
}

TEST_F(DevicetreeBootShimTest, DevicetreeItemWithNoAlias) {
  std::array<std::byte, 256> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());
  boot_shim::DevicetreeBootShim<DevicetreeItem2> shim("devicetree-boot-shim-test", fdt());
  ASSERT_TRUE(shim.Init());

  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  CheckZbiHasItemWithContent(image, ZBI_TYPE_CMDLINE, "--visit-count-b=2");
}

TEST_F(DevicetreeBootShimTest, MultipleDevicetreeItems) {
  std::array<std::byte, 256> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());
  boot_shim::DevicetreeBootShim<DevicetreeItem1, DevicetreeItem2> shim("devicetree-boot-shim-test",
                                                                       fdt());
  ASSERT_TRUE(shim.Init());

  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  CheckZbiHasItemWithContent(image, ZBI_TYPE_CMDLINE, "--visit-count=1");
  CheckZbiHasItemWithContent(image, ZBI_TYPE_CMDLINE, "--visit-count-b=2");
}

TEST_F(DevicetreeBootShimTest, MultipleDevicetreeItemsWithNonDeviceTreeItems) {
  std::array<std::byte, 256> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());
  boot_shim::DevicetreeBootShim<DevicetreeItem1, DevicetreeItem2, NonDeviceTreeItem> shim(
      "devicetree-boot-shim-test", fdt());
  ASSERT_TRUE(shim.Init());

  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  CheckZbiHasItemWithContent(image, ZBI_TYPE_CMDLINE, "--visit-count=1");
  CheckZbiHasItemWithContent(image, ZBI_TYPE_CMDLINE, "--visit-count-b=2");
}

TEST_F(DevicetreeBootShimTest, ItemsWithoutMatchingNodes) {
  std::array<std::byte, 256> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  std::vector<void*> allocs;
  ASSERT_TRUE(image.clear().is_ok());
  // There are no nodes in the synthetic DTB that will match any real matcher, so they will produce
  // NOTHING, but the matchers described in here will still produce items and they will
  // not disturb each other.
  boot_shim::DevicetreeBootShim<
      DevicetreeItem1, DevicetreeItem2, NonDeviceTreeItem, boot_shim::UartItem<>,
      boot_shim::ArmDevicetreePsciItem, boot_shim::ArmDevicetreeGicItem,
      boot_shim::ArmDevicetreeCpuTopologyItem<ArmZeroMpidr>, boot_shim::ArmDevicetreeTimerItem,
      boot_shim::RiscvDevicetreePlicItem, boot_shim::RiscvDevicetreeTimerItem>
      shim("devicetree-boot-shim-test", fdt());
  shim.set_allocator([&allocs](size_t size, size_t alignment, fbl::AllocChecker& ac) -> void* {
    // Custom aligned_alloc since OS X doesnt support it in some versions.
    void* alloc = malloc(size + alignment);
    allocs.push_back(alloc);
    ac.arm(size + alignment, alloc != nullptr);
    return reinterpret_cast<void*>((reinterpret_cast<uintptr_t>(alloc) + alignment) &
                                   ~(alignment - 1));
  });
  ASSERT_TRUE(shim.Init());

  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  CheckZbiHasItemWithContent(image, ZBI_TYPE_CMDLINE, "--visit-count=1");
  CheckZbiHasItemWithContent(image, ZBI_TYPE_CMDLINE, "--visit-count-b=2");
}

}  // namespace
