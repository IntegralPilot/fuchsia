// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree.h>
#include <lib/boot-shim/testing/devicetree-test-fixture.h>
#include <lib/boot-shim/uart.h>
#include <lib/fit/defer.h>

namespace {

using devicetree::testing::LoadDtb;
using devicetree::testing::LoadedDtb;

class DevicetreeSerialNumberItemTest
    : public boot_shim::testing::TestMixin<boot_shim::testing::RiscvDevicetreeTest,
                                           boot_shim::testing::ArmDevicetreeTest,
                                           boot_shim::testing::SyntheticDevicetreeTest> {
 public:
  static void SetUpTestSuite() {
    Mixin::SetUpTestSuite();
    auto loaded_dtb = LoadDtb("serial_number.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    serial_number_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("serial_number_bootargs.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    serial_number_bootargs_ = std::move(loaded_dtb).value();
  }

  static void TearDownTestSuite() { Mixin::TearDownTestSuite(); }

  devicetree::Devicetree serial_number() const { return serial_number_->fdt(); }
  devicetree::Devicetree serial_number_bootargs() const { return serial_number_bootargs_->fdt(); }

 private:
  static std::optional<LoadedDtb> serial_number_;
  static std::optional<LoadedDtb> serial_number_bootargs_;
};

std::optional<LoadedDtb> DevicetreeSerialNumberItemTest::serial_number_ = std::nullopt;
std::optional<LoadedDtb> DevicetreeSerialNumberItemTest::serial_number_bootargs_ = std::nullopt;

TEST_F(DevicetreeSerialNumberItemTest, MissingNode) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = empty_fdt();
  boot_shim::DevicetreeBootShim<boot_shim::RiscvDevicetreeTimerItem> shim("test", fdt);
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  for (auto [header, payload] : image) {
    EXPECT_FALSE(header->type == ZBI_TYPE_SERIAL_NUMBER);
  }
}

TEST_F(DevicetreeSerialNumberItemTest, SerialNumberProperty) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = serial_number();
  boot_shim::DevicetreeBootShim<boot_shim::DevicetreeSerialNumberItem> shim("test", fdt);
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_SERIAL_NUMBER) {
      std::string_view serial = {reinterpret_cast<const char*>(payload.data()), payload.size()};
      EXPECT_TRUE(serial == "foo-bar-baz");
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(DevicetreeSerialNumberItemTest, SerialNumberBootargs) {
  constexpr std::string_view kCmdline = "   androidboot.serialno=foo-bar-baz   ";
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = serial_number_bootargs();
  boot_shim::DevicetreeBootShim<boot_shim::DevicetreeSerialNumberItem> shim("test", fdt);
  shim.set_cmdline(kCmdline);
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_SERIAL_NUMBER) {
      std::string_view serial = {reinterpret_cast<const char*>(payload.data()), payload.size()};
      EXPECT_STREQ(serial, "foo-bar-baz");
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(DevicetreeSerialNumberItemTest, SerialNumberSmallCmdline) {
  constexpr std::string_view kCmdline = "small=1";
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = empty_fdt();
  boot_shim::DevicetreeBootShim<boot_shim::DevicetreeSerialNumberItem> shim("test", fdt);
  shim.set_cmdline(kCmdline);
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_SERIAL_NUMBER) {
      present = true;
    }
  }
  ASSERT_FALSE(present);
}

TEST_F(DevicetreeSerialNumberItemTest, BananaPiF3) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = banana_pi_f3();
  boot_shim::DevicetreeBootShim<boot_shim::DevicetreeSerialNumberItem> shim("test", fdt);
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_SERIAL_NUMBER) {
      std::string_view serial = {reinterpret_cast<const char*>(payload.data()), payload.size()};
      EXPECT_STREQ(serial, "19b6d4a44263");
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(DevicetreeSerialNumberItemTest, KhadasVim3) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = khadas_vim3();
  boot_shim::DevicetreeBootShim<boot_shim::DevicetreeSerialNumberItem> shim("test", fdt);
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_SERIAL_NUMBER) {
      std::string_view serial = {reinterpret_cast<const char*>(payload.data()), payload.size()};
      EXPECT_STREQ(serial, "06ECB1E62CB2");
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(DevicetreeSerialNumberItemTest, SifiveHifiveUnmatched) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = sifive_hifive_unmatched();
  boot_shim::DevicetreeBootShim<boot_shim::DevicetreeSerialNumberItem> shim("test", fdt);
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_SERIAL_NUMBER) {
      std::string_view serial = {reinterpret_cast<const char*>(payload.data()), payload.size()};
      EXPECT_STREQ(serial, "SF105SZ212500140");
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

TEST_F(DevicetreeSerialNumberItemTest, VisionFive2) {
  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  auto fdt = vision_five_2();
  boot_shim::DevicetreeBootShim<boot_shim::DevicetreeSerialNumberItem> shim("test", fdt);
  ASSERT_TRUE(shim.Init());
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  auto clear_err = fit::defer([&]() { image.ignore_error(); });
  bool present = false;
  for (auto [header, payload] : image) {
    if (header->type == ZBI_TYPE_SERIAL_NUMBER) {
      std::string_view serial = {reinterpret_cast<const char*>(payload.data()), payload.size()};
      EXPECT_STREQ(serial, "VF7110B1-2253-D008E000-00002212");
      present = true;
    }
  }
  ASSERT_TRUE(present);
}

}  // namespace
