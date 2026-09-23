// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree.h>
#include <lib/boot-shim/testing/devicetree-test-fixture.h>
#include <lib/boot-shim/uart.h>
#include <lib/fit/defer.h>

#include <cstddef>

#include "lib/devicetree/devicetree.h"
#include "lib/devicetree/testing/loaded-dtb.h"
#include "lib/zbi-format/secure-entropy.h"
#include "zxtest/zxtest.h"

namespace {

using devicetree::testing::LoadDtb;
using devicetree::testing::LoadedDtb;

using EntropyPayload = devicetree::PropEncodedArray<devicetree::PropEncodedArrayElement<1>>;

class DevicetreeSecureEntropyItemTest
    : public boot_shim::testing::TestMixin<boot_shim::testing::SyntheticDevicetreeTest> {
 public:
  static void SetUpTestSuite() {
    Mixin::SetUpTestSuite();
    auto loaded_dtb = LoadDtb("chosen.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    no_rng_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("chosen_with_kaslr_only.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    kaslr_only_dtb_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("chosen_with_rng_only.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    rng_only_dtb_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("chosen_with_kaslr_and_rng.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    kaslr_and_rng_dtb_ = std::move(loaded_dtb).value();
  }

  static void TearDownTestSuite() {
    kaslr_only_dtb_ = std::nullopt;
    rng_only_dtb_ = std::nullopt;
    kaslr_and_rng_dtb_ = std::nullopt;
    Mixin::TearDownTestSuite();
  }

  // These tests are destructive, return a copy of the dtb read from disk.
  LoadedDtb kaslr_only() { return *kaslr_only_dtb_; }
  LoadedDtb rng_only() { return *rng_only_dtb_; }
  LoadedDtb kaslr_and_rng() { return *kaslr_and_rng_dtb_; }
  LoadedDtb no_rng() { return *no_rng_; }

 private:
  static std::optional<LoadedDtb> no_rng_;
  static std::optional<LoadedDtb> kaslr_only_dtb_;
  static std::optional<LoadedDtb> rng_only_dtb_;
  static std::optional<LoadedDtb> kaslr_and_rng_dtb_;
};

std::optional<LoadedDtb> DevicetreeSecureEntropyItemTest::no_rng_ = std::nullopt;
std::optional<LoadedDtb> DevicetreeSecureEntropyItemTest::kaslr_only_dtb_ = std::nullopt;
std::optional<LoadedDtb> DevicetreeSecureEntropyItemTest::rng_only_dtb_ = std::nullopt;
std::optional<LoadedDtb> DevicetreeSecureEntropyItemTest::kaslr_and_rng_dtb_ = std::nullopt;

template <std::convertible_to<std::string_view>... Props>
void CheckPropertyValueCleansed(devicetree::Devicetree devicetree, Props&&... props) {
  devicetree.Walk([&props...](const devicetree::NodePath& path,
                              const devicetree::PropertyDecoder& decoder) -> bool {
    if (path == "/") {
      return true;
    }

    if (path == "/chosen") {
      auto check_prop = [&decoder](std::string_view prop_name) {
        SCOPED_TRACE(std::string(prop_name));
        auto prop_bytes = decoder.FindProperty(prop_name);
        ASSERT_TRUE(prop_bytes);
        for (auto b : prop_bytes->AsBytes()) {
          EXPECT_EQ(b, boot_shim::DevicetreeSecureEntropyItem::kFillPattern);
        }
      };

      (check_prop(props), ...);
    }
    return false;
  });
}

TEST_F(DevicetreeSecureEntropyItemTest, NoRng) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto loaded_dtb = no_rng();
  boot_shim::DevicetreeBootShim<boot_shim::DevicetreeSecureEntropyItem> shim("test",
                                                                             loaded_dtb.fdt());
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_SECURE_ENTROPY) {
      present = true;
    }
  }
  ASSERT_FALSE(present);
}

TEST_F(DevicetreeSecureEntropyItemTest, KaslrOnly) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto loaded_dtb = kaslr_only();
  boot_shim::DevicetreeBootShim<boot_shim::DevicetreeSecureEntropyItem> shim("test",
                                                                             loaded_dtb.fdt());
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  size_t count = 0;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_SECURE_ENTROPY) {
      ASSERT_EQ(header->extra, ZBI_SECURE_ENTROPY_EARLY_BOOT);
      EXPECT_EQ(payload.size_bytes(), 8);
      devicetree::ByteView bytes(reinterpret_cast<uint8_t*>(payload.data()), payload.size_bytes());
      // One cell per element.
      EntropyPayload payload_cells(bytes, 1);
      ASSERT_EQ(payload_cells.size(), 2);
      EXPECT_EQ(payload_cells[0][0], 0xDEADu);
      EXPECT_EQ(payload_cells[1][0], 0xBEEFu);

      // Check that the devicetree kaslr property has been zeroed.
      CheckPropertyValueCleansed(loaded_dtb.fdt(), "kaslr-seed");
      count++;
    }
  }
  ASSERT_EQ(count, 1);
}

TEST_F(DevicetreeSecureEntropyItemTest, RngOnly) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto loaded_dtb = rng_only();
  boot_shim::DevicetreeBootShim<boot_shim::DevicetreeSecureEntropyItem> shim("test",
                                                                             loaded_dtb.fdt());
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  size_t count = 0;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_SECURE_ENTROPY) {
      ASSERT_EQ(header->extra, ZBI_SECURE_ENTROPY_GENERAL);
      EXPECT_EQ(payload.size_bytes(), 8);
      devicetree::ByteView bytes(reinterpret_cast<uint8_t*>(payload.data()), payload.size_bytes());
      // One cell per element.
      EntropyPayload payload_cells(bytes, 1);
      ASSERT_EQ(payload_cells.size(), 2);
      EXPECT_EQ(payload_cells[0][0], 0xDEADu);
      EXPECT_EQ(payload_cells[1][0], 0xBEEFu);

      // Check that the devicetree kaslr property has been zeroed.
      CheckPropertyValueCleansed(loaded_dtb.fdt(), "rng-seed");
      count++;
    }
  }
  ASSERT_EQ(count, 1);
}

TEST_F(DevicetreeSecureEntropyItemTest, KaslrAndRng) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto loaded_dtb = kaslr_and_rng();
  boot_shim::DevicetreeBootShim<boot_shim::DevicetreeSecureEntropyItem> shim("test",
                                                                             loaded_dtb.fdt());
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  size_t rng_count = 0;
  size_t kaslr_count = 0;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_SECURE_ENTROPY) {
      if (header->extra == ZBI_SECURE_ENTROPY_EARLY_BOOT) {
        EXPECT_EQ(payload.size_bytes(), 8);
        devicetree::ByteView bytes(reinterpret_cast<uint8_t*>(payload.data()),
                                   payload.size_bytes());
        // One cell per element.
        EntropyPayload payload_cells(bytes, 1);
        ASSERT_EQ(payload_cells.size(), 2);
        EXPECT_EQ(payload_cells[0][0], 0xDEADu);
        EXPECT_EQ(payload_cells[1][0], 0xBEEFu);

        // Check that the devicetree kaslr property has been zeroed.
        kaslr_count++;
        continue;
      }
      if (header->extra == ZBI_SECURE_ENTROPY_GENERAL) {
        EXPECT_EQ(payload.size_bytes(), 20);
        devicetree::ByteView bytes(reinterpret_cast<uint8_t*>(payload.data()),
                                   payload.size_bytes());
        // One cell per element.
        // 0xDEAD 0xBEEF 0xF00 0xBA8 0xBA5
        EntropyPayload payload_cells(bytes, 1);
        ASSERT_EQ(payload_cells.size(), 5);
        EXPECT_EQ(payload_cells[0][0], 0xDEADu);
        EXPECT_EQ(payload_cells[1][0], 0xBEEFu);
        EXPECT_EQ(payload_cells[2][0], 0xF00u);
        EXPECT_EQ(payload_cells[3][0], 0xBA8u);
        EXPECT_EQ(payload_cells[4][0], 0xBA5u);
        // Check that the devicetree kaslr property has been zeroed.
        rng_count++;
        continue;
      }
    }
  }
  ASSERT_EQ(rng_count, 1);
  ASSERT_EQ(kaslr_count, 1);
  CheckPropertyValueCleansed(loaded_dtb.fdt(), "rng-seed", "kaslr-seed");
}

}  // namespace
