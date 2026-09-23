// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/uart.h>
#include <lib/uart/all.h>
#include <lib/uart/ns8250.h>
#include <lib/uart/null.h>
#include <lib/zbi-format/driver-config.h>
#include <lib/zbitl/image.h>
#include <lib/zbitl/item.h>

#include <array>

#include <zxtest/zxtest.h>

namespace {

using boot_shim::UartItem;

TEST(UartItemTest, SetItemWithNullDriverIsNoOp) {
  std::array<std::byte, 2 * zbitl::AlignedItemLength(sizeof(uart::null::Driver().config()))> data;
  zbitl::Image<std::span<std::byte>> image(data);
  ASSERT_TRUE(image.clear().is_ok());
  size_t clear_image_size = image.size_bytes();

  UartItem item;
  uart::all::Driver dcfg = uart::null::Driver();
  item.Init(uart::all::GetConfig(dcfg));

  ASSERT_TRUE(item.AppendItems(image).is_ok());
  EXPECT_EQ(image.size_bytes(), clear_image_size);
  ASSERT_TRUE(image.take_error().is_ok());
}

TEST(UartItemTest, SetItemWithMmioDriver) {
  std::array<std::byte, 2 * zbitl::AlignedItemLength(sizeof(uart::ns8250::Mmio32Driver().config()))>
      data;
  zbitl::Image<std::span<std::byte>> image(data);
  ASSERT_TRUE(image.clear().is_ok());

  UartItem item;
  using uart_t = uart::ns8250::Mmio32Driver;
  uart_t mmio_dcfg(zbi_dcfg_simple_t{.mmio_phys = 0xBEEF, .irq = 0xDEAD});
  uart::all::Driver dcfg = mmio_dcfg;
  item.Init(uart::all::GetConfig(dcfg));

  ASSERT_TRUE(item.AppendItems(image).is_ok());

  auto it = image.find(ZBI_TYPE_KERNEL_DRIVER);
  image.ignore_error();
  ASSERT_NE(it, image.end());

  auto& [header, payload] = *it;
  EXPECT_EQ(header->extra, uart_t::kExtra);
  ASSERT_EQ(header->length, sizeof(uart_t::config_type));

  auto* actual_mmio_dcfg = reinterpret_cast<zbi_dcfg_simple_t*>(payload.data());
  EXPECT_EQ(actual_mmio_dcfg->mmio_phys, mmio_dcfg.config().mmio_phys);
  EXPECT_EQ(actual_mmio_dcfg->irq, mmio_dcfg.config().irq);
}

#if defined(__x86_64__) || defined(__i386__)
TEST(UartItemTest, SetItemWithPioDriver) {
  std::array<std::byte, 2 * zbitl::AlignedItemLength(sizeof(uart::ns8250::PioDriver().config()))>
      data;
  zbitl::Image<std::span<std::byte>> image(data);
  ASSERT_TRUE(image.clear().is_ok());

  UartItem item;
  using uart_t = uart::ns8250::PioDriver;
  uart_t pio_cfg(zbi_dcfg_simple_pio_t{.base = 0xA110, .irq = 0x5A02});

  uart::all::Driver dcfg = pio_cfg;
  item.Init(uart::all::GetConfig(dcfg));

  ASSERT_TRUE(item.AppendItems(image).is_ok());

  auto it = image.find(ZBI_TYPE_KERNEL_DRIVER);
  image.ignore_error();
  ASSERT_NE(it, image.end());

  auto& [header, payload] = *it;
  EXPECT_EQ(header->extra, uart_t::kExtra);
  ASSERT_EQ(header->length, sizeof(uart_t::config_type));

  auto* actual_pio_dcfg = reinterpret_cast<zbi_dcfg_simple_pio_t*>(payload.data());
  EXPECT_EQ(actual_pio_dcfg->base, pio_cfg.config().base);
  EXPECT_EQ(actual_pio_dcfg->irq, pio_cfg.config().irq);
}
#endif

}  // namespace
