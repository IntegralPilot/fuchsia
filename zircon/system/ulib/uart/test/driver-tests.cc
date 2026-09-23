// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/devicetree/devicetree.h>
#include <lib/stdcompat/array.h>
#include <lib/stdcompat/span.h>
#include <lib/uart/all.h>
#include <lib/uart/amlogic.h>
#include <lib/uart/mock.h>
#include <lib/uart/ns8250.h>
#include <lib/uart/null.h>
#include <lib/uart/pl011.h>
#include <lib/uart/uart.h>
#include <lib/zbi-format/driver-config.h>

#include <array>
#include <string_view>
#include <type_traits>

#include <zxtest/zxtest.h>

using namespace std::literals;

namespace {

TEST(UartTests, Nonblocking) {
  uart::mock::Driver uart;

  uart.ExpectLock()
      .ExpectInit()
      .ExpectUnlock()
      // First Write call -> sends all chars, no waiting.
      .ExpectLock()
      .ExpectTxReady(true)
      .ExpectWrite("hi!"sv)
      .ExpectUnlock()
      // Second Write call -> sends half, then waits.
      .ExpectLock()
      .ExpectTxReady(true)
      .ExpectWrite("hello "sv)
      .ExpectTxReady(false)
      .ExpectWait(false)
      .ExpectTxReady(true)
      .ExpectWrite("world\r\n"sv)
      .ExpectUnlock();

  uart::KernelDriver<uart::mock::Driver, uart::mock::IoProvider, uart::mock::SyncPolicy,
                     uart::mock::IrqProvider>
      driver(std::move(uart));

  driver.Init<uart::mock::Locking>();
  EXPECT_EQ(driver.Write<uart::mock::Locking>("hi!"), 3);
  EXPECT_EQ(driver.Write<uart::mock::Locking>("hello world\n"), 12);
}

TEST(UartTests, LockPolicy) {
  uart::mock::Driver uart;
  uart.ExpectLock()
      .ExpectInit()
      .ExpectUnlock()
      // First Write call -> sends all chars, no waiting.
      .ExpectTxReady(true)
      .ExpectWrite("hi!"sv)
      // Second Write call -> sends half, then waits.
      .ExpectTxReady(true)
      .ExpectWrite("hello "sv)
      .ExpectTxReady(false)
      .ExpectWait(false)
      .ExpectTxReady(true)
      .ExpectWrite("world\r\n"sv);

  uart::KernelDriver<uart::mock::Driver, uart::mock::IoProvider, uart::mock::SyncPolicy,
                     uart::mock::IrqProvider>
      driver(std::move(uart));

  driver.Init<uart::mock::Locking>();
  // Just check that lock args are forwarded correctly.
  EXPECT_EQ(driver.Write<uart::mock::NoopLocking>("hi!"), 3);
  EXPECT_EQ(driver.Write<uart::mock::NoopLocking>("hello world\n"), 12);
}

TEST(UartTests, Blocking) {
  uart::mock::Driver uart;

  uart.ExpectLock()
      .ExpectInit()
      .ExpectUnlock()
      // First Write call -> sends all chars, no waiting.
      .ExpectLock()
      .ExpectTxReady(true)
      .ExpectWrite("hi!"sv)
      .ExpectUnlock()
      // Second Write call -> sends half, then waits.
      .ExpectLock()
      .ExpectTxReady(true)
      .ExpectWrite("hello "sv)
      .ExpectTxReady(false)
      .ExpectWait(true)
      .ExpectAssertHeld()
      .ExpectEnableTxInterrupt()
      .ExpectTxReady(true)
      .ExpectWrite("world\r\n"sv)
      .ExpectUnlock();

  uart::KernelDriver<uart::mock::Driver, uart::mock::IoProvider, uart::mock::SyncPolicy,
                     uart::mock::IrqProvider>
      driver(std::move(uart));

  driver.Init<uart::mock::Locking>();
  EXPECT_EQ(driver.Write<uart::mock::Locking>("hi!"), 3);
  EXPECT_EQ(driver.Write<uart::mock::Locking>("hello world\n"), 12);
}

TEST(UartTests, Config) {
  uart::all::Config all_configs;

  zbi_dcfg_simple_t dcfg = {
      .mmio_phys = 1,
      .irq = 2,
      .flags = 3,
  };

  {
    zbi_dcfg_simple_t dcfg2 = {
        .mmio_phys = 2,
        .irq = 2,
        .flags = 3,
    };
    uart::Config<uart::ns8250::Mmio32Driver> cfg1{dcfg};
    uart::Config<uart::ns8250::Mmio32Driver> cfg2{dcfg};
    uart::Config<uart::ns8250::Mmio32Driver> cfg3{dcfg2};

    zbi_dcfg_simple_pio_t dcfg_pio = {
        .base = 123,
        .irq = 4,
    };
    zbi_dcfg_simple_pio_t dcfg_pio2 = {
        .base = 124,
        .irq = 4,
    };

    uart::Config<uart::ns8250::PioDriver> cfg4{dcfg_pio};
    uart::Config<uart::ns8250::PioDriver> cfg5{dcfg_pio};
    uart::Config<uart::ns8250::PioDriver> cfg6{dcfg_pio2};
    uart::Config<uart::null::Driver> cfg7{};
    uart::Config<uart::null::Driver> cfg8{};
    uart::Config<uart::ns8250::Mmio8Driver> cfg9{dcfg};

    // Check operator== and !=.

    // MMIO
    EXPECT_EQ(cfg1, cfg1);
    EXPECT_EQ(cfg1, cfg2);
    EXPECT_EQ(cfg2, cfg1);
    EXPECT_NE(cfg1, cfg3);
    EXPECT_NE(cfg1, cfg4);

    // MMIO vs other types
    EXPECT_NE(cfg1, cfg7);
    EXPECT_NE(cfg1, cfg9);
    EXPECT_NE(cfg7, cfg1);
    EXPECT_NE(cfg9, cfg1);

    // PIO
    EXPECT_EQ(cfg4, cfg4);
    EXPECT_EQ(cfg4, cfg5);
    EXPECT_EQ(cfg5, cfg4);
    EXPECT_NE(cfg4, cfg6);

    // Stub driver
    EXPECT_EQ(cfg7, cfg7);
    EXPECT_EQ(cfg7, cfg8);
  }

  // uart::foo::Driver
  uart::ns8250::Mmio32Driver uart(dcfg);

  // From uart driver.
  all_configs = uart;
  all_configs.Visit([&dcfg](auto& config) {
    if constexpr (std::is_same_v<std::decay_t<decltype(*config)>, zbi_dcfg_simple_t>) {
      using uart_type = typename std::decay_t<decltype(config)>::uart_type;
      ASSERT_TRUE((std::is_same_v<uart_type, uart::ns8250::Mmio32Driver>));
      EXPECT_EQ(config->mmio_phys, dcfg.mmio_phys);
      EXPECT_EQ(config->irq, dcfg.irq);
      EXPECT_EQ(config->flags, dcfg.flags);
    } else {
      FAIL("Unexpected configuration");
    }
  });

  // From kernel driver
  zbi_dcfg_simple_pio_t pio_cfg{
      .base = 0x3f8,
      .irq = 3,
  };
  uart::KernelDriver<uart::ns8250::PioDriver, uart::mock::IoProvider, uart::UnsynchronizedPolicy,
                     uart::mock::IrqProvider>
      driver(pio_cfg);
  all_configs = driver;
  all_configs.Visit([&pio_cfg](auto& config) {
    if constexpr (std::is_same_v<std::decay_t<decltype(*config)>, zbi_dcfg_simple_pio_t>) {
      using uart_type = typename std::decay_t<decltype(config)>::uart_type;
      ASSERT_TRUE((std::is_same_v<uart_type, uart::ns8250::PioDriver>));
      EXPECT_EQ(config->base, pio_cfg.base);
      EXPECT_EQ(config->irq, pio_cfg.irq);
    } else {
      FAIL("Unexpected configuration");
    }
  });

  // Construct from uart.
  {
    zbi_dcfg_simple_t dcfg = {
        .mmio_phys = 1,
        .irq = 2,
        .flags = 3,
    };

    // uart::foo::Driver
    uart::ns8250::Mmio32Driver uart(dcfg);
    uart::all::Config all_configs(uart);

    all_configs.Visit([&dcfg](auto& config) {
      if constexpr (std::is_same_v<std::decay_t<decltype(*config)>, zbi_dcfg_simple_t>) {
        using uart_type = typename std::decay_t<decltype(config)>::uart_type;
        ASSERT_TRUE((std::is_same_v<uart_type, uart::ns8250::Mmio32Driver>));
        EXPECT_EQ(config->mmio_phys, dcfg.mmio_phys);
        EXPECT_EQ(config->irq, dcfg.irq);
        EXPECT_EQ(config->flags, dcfg.flags);
      } else {
        FAIL("Unexpected configuration");
      }
    });
  }

  // Construct from kernel driver.
  {
    // From kernel driver
    zbi_dcfg_simple_pio_t pio_cfg{
        .base = 0x3f8,
        .irq = 3,
    };
    uart::KernelDriver<uart::ns8250::PioDriver, uart::mock::IoProvider, uart::UnsynchronizedPolicy,
                       uart::mock::IrqProvider>
        driver(pio_cfg);
    uart::all::Config all_configs(driver);
    all_configs.Visit([&pio_cfg](auto& config) {
      if constexpr (std::is_same_v<std::decay_t<decltype(*config)>, zbi_dcfg_simple_pio_t>) {
        using uart_type = typename std::decay_t<decltype(config)>::uart_type;
        ASSERT_TRUE((std::is_same_v<uart_type, uart::ns8250::PioDriver>));
        EXPECT_EQ(config->base, pio_cfg.base);
        EXPECT_EQ(config->irq, pio_cfg.irq);
      } else {
        FAIL("Unexpected configuration");
      }
    });
  }
}

TEST(UartTests, AllConfig) {
  // Assignment
  uart::all::KernelDriver<uart::mock::IoProvider, uart::UnsynchronizedPolicy,
                          uart::mock::IrqProvider>
      all_driver;
  EXPECT_FALSE(all_driver);
  zbi_dcfg_simple_pio_t pio_cfg{
      .base = 0x3f8,
      .irq = 3,
  };
  uart::KernelDriver<uart::ns8250::PioDriver, uart::mock::IoProvider, uart::UnsynchronizedPolicy,
                     uart::mock::IrqProvider>
      driver(pio_cfg);
  all_driver = std::move(driver).TakeUart();
  EXPECT_TRUE(all_driver);

  uart::all::Config all_config = all_driver.config();
  all_config.Visit([&pio_cfg](auto& config) {
    if constexpr (std::is_same_v<std::decay_t<decltype(*config)>, zbi_dcfg_simple_pio_t>) {
      using uart_type = typename std::decay_t<decltype(config)>::uart_type;
      ASSERT_TRUE((std::is_same_v<uart_type, uart::ns8250::PioDriver>));
      EXPECT_EQ(config->base, pio_cfg.base);
      EXPECT_EQ(config->irq, pio_cfg.irq);
    } else {
      FAIL("Unexpected configuration");
    }
  });

  // Constctor
  uart::all::KernelDriver<uart::mock::IoProvider, uart::UnsynchronizedPolicy,
                          uart::mock::IrqProvider>
      all_driver2(all_config);
  all_driver2.Visit([&](const auto& driver) {
    using uart_type = typename std::decay_t<decltype(driver)>::uart_type;
    if constexpr (std::is_same_v<uart_type, uart::ns8250::PioDriver>) {
      EXPECT_EQ(driver.config().base, pio_cfg.base);
      EXPECT_EQ(driver.config().irq, pio_cfg.irq);
    } else {
      FAIL("Unexpected configuration");
    }
  });
}

TEST(UartTests, Null) {
  uart::KernelDriver<uart::null::Driver, uart::mock::IoProvider, uart::UnsynchronizedPolicy,
                     uart::mock::IrqProvider>
      driver;
  // Unsynchronized LockPolicy is dropped.
  driver.Init();
  EXPECT_EQ(driver.Write("hi!"), 3);
  EXPECT_EQ(driver.Write("hello world\n"), 12);
  EXPECT_FALSE(driver.Read());
}

TEST(UartTests, All) {
  using AllConfig = uart::all::Config<>;
  using AllDriver = uart::all::KernelDriver<uart::mock::IoProvider, uart::UnsynchronizedPolicy,
                                            uart::mock::IrqProvider>;

  // Default constructed `configuration` should default to being the `null::Driver`.
  AllDriver driver(AllConfig{});
  EXPECT_FALSE(driver);
  // Make sure Unparse is instantiated.
  driver.Unparse();

  // Use selected driver.
  driver.Visit([](auto&& driver) {
    driver.Init();
    EXPECT_EQ(driver.Write("hi!"), 3);
  });

  // Transfer state to a new instantiation and pick up using it.
  AllDriver newdriver{std::move(driver).TakeUart()};

  EXPECT_FALSE(driver);
  EXPECT_FALSE(newdriver);
  newdriver.Visit([](auto&& driver) {
    EXPECT_EQ(driver.Write("hello world\n"), 12);
    EXPECT_FALSE(driver.Read());
  });
}

auto as_uint8 = [](auto& val) {
  using byte_type = std::conditional_t<std::is_const_v<std::remove_reference_t<decltype(val)>>,
                                       const uint8_t, uint8_t>;
  return cpp20::span<byte_type>(reinterpret_cast<byte_type*>(&val), sizeof(val));
};

auto append = [](auto& vec, auto&& other) { vec.insert(vec.end(), other.begin(), other.end()); };

uint32_t byte_swap(uint32_t val) {
  if constexpr (cpp20::endian::native == cpp20::endian::big) {
    return val;
  } else {
    auto bytes = as_uint8(val);
    return static_cast<uint32_t>(bytes[0]) << 24 | static_cast<uint32_t>(bytes[1]) << 16 |
           static_cast<uint32_t>(bytes[2]) << 8 | static_cast<uint32_t>(bytes[3]);
  }
}

// Small helper so we can verify the behavior of CachedProperties.
struct PropertyBuilder {
  devicetree::Properties Build() {
    return devicetree::Properties(
        {property_block.data(), property_block.size()},
        std::string_view(reinterpret_cast<const char*>(string_block.data()), string_block.size()));
  }

  void Add(std::string_view name, uint32_t value) {
    uint32_t name_off = byte_swap(static_cast<uint32_t>(string_block.size()));
    // String must be null terminated.
    append(string_block, name);
    string_block.push_back('\0');

    uint32_t len = byte_swap(sizeof(uint32_t));

    if (!property_block.empty()) {
      const uint32_t kFdtPropToken = byte_swap(0x00000003);
      append(property_block, as_uint8(kFdtPropToken));
    }
    // this are all 32b aliagned, no padding need.
    append(property_block, as_uint8(len));
    append(property_block, as_uint8(name_off));
    uint32_t be_value = byte_swap(value);
    append(property_block, as_uint8(be_value));
  }

  void Add(std::string_view key, cpp20::span<const std::string_view> str_list) {
    constexpr std::array<uint8_t, sizeof(uint32_t) - 1> kPadding = {};

    uint32_t name_off = byte_swap(static_cast<uint32_t>(string_block.size()));
    // String must be null terminated.
    append(string_block, key);
    string_block.push_back('\0');

    // Add the null terminator.
    uint32_t len = 0;
    for (std::string_view str : str_list) {
      // Include terminator.
      len += str.length() + 1;
    }

    cpp20::span<const uint8_t> padding;
    if (auto remainder = len % sizeof(uint32_t); remainder != 0) {
      padding = cpp20::span(kPadding).subspan(0, sizeof(uint32_t) - remainder);
    }
    len = byte_swap(len);

    if (!property_block.empty()) {
      const uint32_t kFdtPropToken = byte_swap(0x00000003);
      append(property_block, as_uint8(kFdtPropToken));
    }
    append(property_block, as_uint8(len));
    append(property_block, as_uint8(name_off));
    for (auto str : str_list) {
      append(property_block, str);
      property_block.push_back('\0');
    }
    append(property_block, padding);
  }

  std::vector<uint8_t> property_block;
  std::vector<uint8_t> string_block;
};

TEST(UartTests, ConfigSelect) {
  using AllDrivers = std::variant<uart::null::Driver, uart::pl011::Driver, uart::dw8250::Driver,
                                  uart::ns8250::Mmio32Driver, uart::ns8250::Mmio8Driver,
                                  uart::ns8250::PioDriver, uart::amlogic::Driver>;
  using AllConfigs = uart::all::Config<AllDrivers>;

  static auto check_zeroed = []<typename ConfigType>(const ConfigType& cfg) {
    if constexpr (std::is_same_v<ConfigType, zbi_dcfg_simple_t>) {
      EXPECT_EQ(cfg.mmio_phys, 0);
      EXPECT_EQ(cfg.irq, 0);
      EXPECT_EQ(cfg.flags, 0);
    } else if constexpr (std::is_same_v<ConfigType, zbi_dcfg_simple_pio_t>) {
      EXPECT_EQ(cfg.base, 0);
      EXPECT_EQ(cfg.irq, 0);
    }
  };

  constexpr std::array kCompatibles = {"foo,bar"sv, "ns16550a"sv};
  {
    // Match no reg shift or io width
    PropertyBuilder builder;
    builder.Add("compatible", kCompatibles);
    auto props = builder.Build();
    devicetree::PropertyDecoder decoder(props);

    std::optional config = AllConfigs::Select(decoder);
    ASSERT_TRUE(config);

    config->Visit([]<typename UartDriver>(const uart::Config<UartDriver>& cfg) {
      ASSERT_TRUE((std::is_same_v<UartDriver, uart::ns8250::Mmio8Driver>));
      check_zeroed(*cfg);
    });
  }

  // Match with reg shift and io width
  {
    // Match no reg shift or io width
    PropertyBuilder builder;
    builder.Add("compatible", kCompatibles);
    builder.Add("reg-shift", 2);
    builder.Add("reg-io-width", 4);
    auto props = builder.Build();
    devicetree::PropertyDecoder decoder(props);

    std::optional config = AllConfigs::Select(decoder);
    ASSERT_TRUE(config);

    config->Visit([]<typename UartDriver>(const uart::Config<UartDriver>& cfg) {
      ASSERT_TRUE((std::is_same_v<UartDriver, uart::ns8250::Mmio32Driver>));
      check_zeroed(*cfg);
    });
  }

  // Match Dw8250
  constexpr std::array kDwCompatibles = {"foo,bar"sv, "snps,dw-apb-uart"sv,
                                         "goog,goog-dw-apb-uart"sv};
  {
    PropertyBuilder builder;
    builder.Add("compatible", kDwCompatibles);
    auto props = builder.Build();
    devicetree::PropertyDecoder decoder(props);

    std::optional config = AllConfigs::Select(decoder);
    ASSERT_FALSE(config);
  }

  {
    // Match no reg shift or io width
    PropertyBuilder builder;
    builder.Add("compatible", kDwCompatibles);
    builder.Add("reg-shift", 2);
    builder.Add("reg-io-width", 4);
    auto props = builder.Build();
    devicetree::PropertyDecoder decoder(props);

    std::optional config = AllConfigs::Select(decoder);
    ASSERT_TRUE(config);

    config->Visit([]<typename UartDriver>(const uart::Config<UartDriver>& cfg) {
      ASSERT_TRUE((std::is_same_v<UartDriver, uart::dw8250::Driver>));
      check_zeroed(*cfg);
    });
  }

  {
    // Match no reg shift or io width
    PropertyBuilder builder;
    builder.Add("compatible", cpp20::to_array<std::string_view>({"foo", "arm,pl011"}));
    auto props = builder.Build();
    devicetree::PropertyDecoder decoder(props);

    std::optional config = AllConfigs::Select(decoder);
    ASSERT_TRUE(config);

    config->Visit([]<typename UartDriver>(const uart::Config<UartDriver>& cfg) {
      ASSERT_TRUE((std::is_same_v<UartDriver, uart::pl011::Driver>));
      check_zeroed(*cfg);
    });
  }

  {
    // Match no reg shift or io width
    PropertyBuilder builder;
    builder.Add("compatible", cpp20::to_array<std::string_view>({"foo", "amlogic,meson-gx-uart"}));
    auto props = builder.Build();
    devicetree::PropertyDecoder decoder(props);

    std::optional config = AllConfigs::Select(decoder);
    ASSERT_TRUE(config);

    config->Visit([]<typename UartDriver>(const uart::Config<UartDriver>& cfg) {
      ASSERT_TRUE((std::is_same_v<UartDriver, uart::amlogic::Driver>));
      check_zeroed(*cfg);
    });
  }
}

}  // namespace
