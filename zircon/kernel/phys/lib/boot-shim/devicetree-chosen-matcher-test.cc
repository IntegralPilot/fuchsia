// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim/devicetree-boot-shim.h>
#include <lib/boot-shim/devicetree.h>
#include <lib/boot-shim/testing/devicetree-test-fixture.h>
#include <lib/fit/defer.h>
#include <lib/uart/amlogic.h>
#include <lib/uart/ns8250.h>
#include <lib/zbi-format/driver-config.h>
#include <lib/zbitl/image.h>

#include <zxtest/zxtest.h>

namespace {
using boot_shim::testing::LoadDtb;
using boot_shim::testing::LoadedDtb;

class ChosenNodeMatcherTest
    : public boot_shim::testing::TestMixin<boot_shim::testing::ArmDevicetreeTest,
                                           boot_shim::testing::RiscvDevicetreeTest> {
 public:
  static void SetUpTestSuite() {
    Mixin::SetUpTestSuite();
    auto loaded_dtb = LoadDtb("chosen.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    chosen_dtb_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("chosen_with_console.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    chosen_with_console_dtb_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("chosen_with_console_aml.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    chosen_with_console_aml_dtb_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("chosen_with_serial_alias.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    chosen_with_serial_alias_dtb_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("chosen_no_uart_interrupts.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    chosen_no_uart_interrupts_dtb_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("chosen_with_console_and_stdout_path.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    chosen_with_console_and_stdout_path_dtb_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("chosen_unknown_intc.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    chosen_unknown_intc_dtb_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("chosen_with_reg_offset.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    chosen_with_reg_offset_dtb_ = std::move(loaded_dtb).value();

    loaded_dtb = LoadDtb("chosen_with_translation.dtb");
    ASSERT_TRUE(loaded_dtb.is_ok(), "%s", loaded_dtb.error_value().c_str());
    chosen_with_translation_dtb_ = std::move(loaded_dtb).value();
  }

  static void TearDownTestSuite() {
    chosen_dtb_ = std::nullopt;
    chosen_with_console_dtb_ = std::nullopt;
    chosen_unknown_intc_dtb_ = std::nullopt;
    chosen_with_reg_offset_dtb_ = std::nullopt;
    chosen_with_translation_dtb_ = std::nullopt;
    chosen_with_console_and_stdout_path_dtb_ = std::nullopt;
    chosen_with_console_aml_dtb_ = std::nullopt;
    chosen_with_serial_alias_dtb_ = std::nullopt;
    chosen_no_uart_interrupts_dtb_ = std::nullopt;
    Mixin::TearDownTestSuite();
  }

  devicetree::Devicetree chosen() { return chosen_dtb_->fdt(); }
  devicetree::Devicetree chosen_with_console() { return chosen_with_console_dtb_->fdt(); }
  devicetree::Devicetree chosen_with_console_aml() { return chosen_with_console_aml_dtb_->fdt(); }
  devicetree::Devicetree chosen_with_serial_alias() { return chosen_with_serial_alias_dtb_->fdt(); }
  devicetree::Devicetree chosen_no_uart_interrupts() {
    return chosen_no_uart_interrupts_dtb_->fdt();
  }
  devicetree::Devicetree chosen_with_console_and_stdout_path() {
    return chosen_with_console_and_stdout_path_dtb_->fdt();
  }
  devicetree::Devicetree chosen_with_reg_offset() { return chosen_with_reg_offset_dtb_->fdt(); }
  devicetree::Devicetree chosen_unknown_intc() { return chosen_unknown_intc_dtb_->fdt(); }
  devicetree::Devicetree chosen_with_translation() { return chosen_with_translation_dtb_->fdt(); }

 private:
  static std::optional<LoadedDtb> chosen_dtb_;
  static std::optional<LoadedDtb> chosen_with_console_dtb_;
  static std::optional<LoadedDtb> chosen_with_console_aml_dtb_;
  static std::optional<LoadedDtb> chosen_with_serial_alias_dtb_;
  static std::optional<LoadedDtb> chosen_no_uart_interrupts_dtb_;
  static std::optional<LoadedDtb> chosen_with_console_and_stdout_path_dtb_;
  static std::optional<LoadedDtb> chosen_with_reg_offset_dtb_;
  static std::optional<LoadedDtb> chosen_unknown_intc_dtb_;
  static std::optional<LoadedDtb> chosen_with_translation_dtb_;
};

std::optional<LoadedDtb> ChosenNodeMatcherTest::chosen_dtb_ = std::nullopt;
std::optional<LoadedDtb> ChosenNodeMatcherTest::chosen_with_console_dtb_ = std::nullopt;
std::optional<LoadedDtb> ChosenNodeMatcherTest::chosen_with_console_aml_dtb_ = std::nullopt;
std::optional<LoadedDtb> ChosenNodeMatcherTest::chosen_with_serial_alias_dtb_ = std::nullopt;
std::optional<LoadedDtb> ChosenNodeMatcherTest::chosen_no_uart_interrupts_dtb_ = std::nullopt;
std::optional<LoadedDtb> ChosenNodeMatcherTest::chosen_with_console_and_stdout_path_dtb_ =
    std::nullopt;
std::optional<LoadedDtb> ChosenNodeMatcherTest::chosen_with_reg_offset_dtb_ = std::nullopt;
std::optional<LoadedDtb> ChosenNodeMatcherTest::chosen_unknown_intc_dtb_ = std::nullopt;
std::optional<LoadedDtb> ChosenNodeMatcherTest::chosen_with_translation_dtb_ = std::nullopt;

struct ExpectedChosen {
  uintptr_t ramdisk_start;
  uintptr_t ramdisk_end;
  std::string_view cmdline;
  std::string_view uart_config_name;
  zbi_dcfg_simple_t uart_config;
  std::string_view uart_absolute_path;
};

using AllUartDrivers = std::variant<uart::null::Driver, uart::dw8250::Driver, uart::pl011::Driver,
                                    uart::ns8250::Mmio8Driver, uart::ns8250::Mmio32Driver,
                                    uart::amlogic::Driver, uart::ns8250::PxaDriver>;

template <typename ChosenItemType>
void CheckChosenMatcher(ChosenItemType& matcher, const ExpectedChosen& expected) {
  std::vector<std::unique_ptr<devicetree::Node>> nodes_in_path;
  size_t current = 0;
  while (current < expected.uart_absolute_path.length()) {
    size_t next = current;
    if (next = expected.uart_absolute_path.substr(current).find('/');
        next == std::string_view::npos) {
      next = expected.uart_absolute_path.length() - current;
    }
    nodes_in_path.push_back(
        std::make_unique<devicetree::Node>(expected.uart_absolute_path.substr(current, next)));
    current += next + 1;
  }

  devicetree::NodePath expected_uart_path;
  auto cleanup = fit::defer([&]() {
    while (!expected_uart_path.is_empty()) {
      expected_uart_path.pop_back();
    }
  });
  for (auto& node : nodes_in_path) {
    expected_uart_path.push_back(node.get());
  }

  // Cmdline Check.
  EXPECT_EQ(matcher.cmdline(), expected.cmdline);

  // Ramdisk captured correctly.
  auto ramdisk = matcher.ramdisk();
  uintptr_t ramdisk_start = reinterpret_cast<uintptr_t>(ramdisk.data());
  EXPECT_EQ(ramdisk_start, expected.ramdisk_start);
  EXPECT_EQ(ramdisk.size(), expected.ramdisk_end - expected.ramdisk_start);

  if (!expected_uart_path.is_empty()) {
    ASSERT_TRUE(matcher.stdout_path());
    EXPECT_EQ(*matcher.stdout_path(), expected_uart_path);
  } else {
    ASSERT_TRUE(!matcher.stdout_path());
  }

  // Uart configuration.
  std::optional driver_config = matcher.uart_config();
  ASSERT_TRUE(driver_config);
  driver_config->Visit([expected]<typename UartType>(const uart::Config<UartType>& config) {
    if constexpr (std::is_same_v<typename UartType::config_type, zbi_dcfg_simple_t>) {
      EXPECT_EQ(UartType::kConfigName, expected.uart_config_name, "Actual name %s\n",
                UartType::kConfigName.data());

      EXPECT_EQ(config->mmio_phys, expected.uart_config.mmio_phys);
      // The bootstrap phase does not decode interrupt.
      EXPECT_EQ(config->irq, expected.uart_config.irq);
      EXPECT_EQ(config->flags, expected.uart_config.flags);
    } else {
      FAIL("Unexpected configuration for driver: %s", fbl::TypeInfo<UartType>::Name());
    }
  });
}

TEST_F(ChosenNodeMatcherTest, Chosen) {
  auto fdt = chosen();
  boot_shim::DevicetreeChosenNodeMatcher<AllUartDrivers> chosen_matcher("test", stdout);

  ASSERT_TRUE(devicetree::Match(fdt, chosen_matcher));

  CheckChosenMatcher(chosen_matcher,
                     {
                         .ramdisk_start = 0x48000000,
                         .ramdisk_end = 0x58000000,
                         .cmdline = "-foo=bar -bar=baz",
                         .uart_config_name = uart::pl011::Driver::kConfigName,
                         .uart_config =
                             {
                                 .mmio_phys = 0x9000000,
                                 .irq = 33,
                                 .flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                                          ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
                             },
                         .uart_absolute_path = "/some-interrupt-controller/pl011uart@9000000",
                     });
}

TEST_F(ChosenNodeMatcherTest, ChosenWithConsole) {
  auto fdt = chosen_with_console();
  boot_shim::DevicetreeChosenNodeMatcher<AllUartDrivers> chosen_matcher("test", stdout);

  ASSERT_TRUE(devicetree::Match(fdt, chosen_matcher));

  CheckChosenMatcher(chosen_matcher,
                     {
                         .ramdisk_start = 0x48000000,
                         .ramdisk_end = 0x58000000,
                         .cmdline = "-foo=bar -bar=baz console=ttyS003",
                         .uart_config_name = uart::pl011::Driver::kConfigName,
                         .uart_config =
                             {
                                 .mmio_phys = 0x9003000,
                                 .irq = 36,
                                 .flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                                          ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
                             },
                         // no stdout-path is set in the chosen node, console should be
                         // used as a fallback.
                         .uart_absolute_path = "",
                     });
}

TEST_F(ChosenNodeMatcherTest, ChosenWithConsoleAndVendor) {
  auto fdt = chosen_with_console_aml();
  boot_shim::DevicetreeChosenNodeMatcher<AllUartDrivers> chosen_matcher("test", stdout);

  ASSERT_TRUE(devicetree::Match(fdt, chosen_matcher));

  CheckChosenMatcher(chosen_matcher,
                     {
                         .ramdisk_start = 0x48000000,
                         .ramdisk_end = 0x58000000,
                         .cmdline = "-foo=bar -bar=baz console=ttyAML002",
                         .uart_config_name = uart::amlogic::Driver::kConfigName,
                         .uart_config =
                             {
                                 .mmio_phys = 0x9003000,
                                 .irq = 36,
                                 .flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                                          ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
                             },
                         // no stdout-path is set in the chosen node, console should be
                         // used as a fallback.
                         .uart_absolute_path = "",
                     });
}

TEST_F(ChosenNodeMatcherTest, ChosenWithSerialAlias) {
  auto fdt = chosen_with_serial_alias();
  boot_shim::DevicetreeChosenNodeMatcher<AllUartDrivers> chosen_matcher("test", stdout);

  ASSERT_TRUE(devicetree::Match(fdt, chosen_matcher));

  CheckChosenMatcher(chosen_matcher,
                     {
                         .ramdisk_start = 0x48000000,
                         .ramdisk_end = 0x58000000,
                         .cmdline = "-foo=bar -bar=baz console=ttyS3",
                         .uart_config_name = uart::pl011::Driver::kConfigName,
                         .uart_config =
                             {
                                 .mmio_phys = 0x9003000,
                                 .irq = 36,
                                 .flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                                          ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
                             },
                         .uart_absolute_path = "/some-interrupt-controller/pl011uart@9003000",
                     });
}

TEST_F(ChosenNodeMatcherTest, ChosenNoUartInterrupts) {
  auto fdt = chosen_no_uart_interrupts();
  boot_shim::DevicetreeChosenNodeMatcher<AllUartDrivers> chosen_matcher("test", stdout);

  ASSERT_TRUE(devicetree::Match(fdt, chosen_matcher));

  CheckChosenMatcher(chosen_matcher,
                     {
                         .ramdisk_start = 0x48000000,
                         .ramdisk_end = 0x58000000,
                         .cmdline = "-foo=bar -bar=baz",
                         .uart_config_name = uart::pl011::Driver::kConfigName,
                         .uart_config =
                             {
                                 .mmio_phys = 0x9000000,
                                 .irq = 0,
                                 .flags = 0,
                             },
                         .uart_absolute_path = "/some-interrupt-controller/pl011uart@9000000",
                     });
}

TEST_F(ChosenNodeMatcherTest, ChosenWithConsoleAndStdout) {
  // In this case, the stdout-path trumps whatever the console argument provides.
  auto fdt = chosen_with_console_and_stdout_path();
  boot_shim::DevicetreeChosenNodeMatcher<AllUartDrivers> chosen_matcher("test", stdout);

  ASSERT_TRUE(devicetree::Match(fdt, chosen_matcher));

  CheckChosenMatcher(chosen_matcher,
                     {
                         .ramdisk_start = 0x48000000,
                         .ramdisk_end = 0x58000000,
                         .cmdline = "-foo=bar -bar=baz console=ttyS003",
                         .uart_config_name = uart::pl011::Driver::kConfigName,
                         .uart_config =
                             {
                                 .mmio_phys = 0x9000000,
                                 .irq = 33,
                                 .flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                                          ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
                             },
                         .uart_absolute_path = "/some-interrupt-controller/pl011uart@9000000",
                     });
}

TEST_F(ChosenNodeMatcherTest, ChosenWithRegOffset) {
  auto fdt = chosen_with_reg_offset();
  boot_shim::DevicetreeChosenNodeMatcher<AllUartDrivers> chosen_matcher("test", stdout);

  ASSERT_TRUE(devicetree::Match(fdt, chosen_matcher));

  CheckChosenMatcher(chosen_matcher,
                     {
                         .ramdisk_start = 0x48000000,
                         .ramdisk_end = 0x58000000,
                         .cmdline = "-foo=bar -bar=baz",
                         .uart_config_name = uart::pl011::Driver::kConfigName,
                         .uart_config =
                             {
                                 .mmio_phys = 0x9000123,
                                 .irq = 33,
                                 .flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                                          ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
                             },
                         .uart_absolute_path = "/some-interrupt-controller/pl011uart@9000000",
                     });
}

TEST_F(ChosenNodeMatcherTest, ChosenWithAddressTranslation) {
  auto fdt = chosen_with_translation();
  boot_shim::DevicetreeChosenNodeMatcher<AllUartDrivers> chosen_matcher("test", stdout);

  ASSERT_TRUE(devicetree::Match(fdt, chosen_matcher));

  CheckChosenMatcher(chosen_matcher,
                     {
                         .ramdisk_start = 0x48000000,
                         .ramdisk_end = 0x58000000,
                         .cmdline = "-foo=bar -bar=baz",
                         .uart_config_name = uart::pl011::Driver::kConfigName,
                         .uart_config =
                             {
                                 .mmio_phys = 0x9030000,
                                 .irq = 33,
                                 .flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                                          ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
                             },
                         .uart_absolute_path = "/some-interrupt-controller@0/pl011uart@9000000",
                     });
}

TEST_F(ChosenNodeMatcherTest, ChosenWithUnknownInterruptController) {
  auto fdt = chosen_unknown_intc();
  boot_shim::DevicetreeChosenNodeMatcher<AllUartDrivers> chosen_matcher("test", stdout);

  ASSERT_TRUE(devicetree::Match(fdt, chosen_matcher));
  CheckChosenMatcher(chosen_matcher,
                     {
                         .ramdisk_start = 0x48000000,
                         .ramdisk_end = 0x58000000,
                         .cmdline = "-foo=bar -bar=baz",
                         .uart_config_name = uart::pl011::Driver::kConfigName,
                         .uart_config =
                             {
                                 .mmio_phys = 0x9000000,
                                 .irq = 0,
                                 .flags = 0,
                             },
                         .uart_absolute_path = "/some-interrupt-controller/pl011uart@9000000",
                     });
}

TEST_F(ChosenNodeMatcherTest, CrosvmArm) {
  constexpr std::string_view kCmdline =
      "panic=-1 kernel.experimental.serial_migration=true console.shell=true "
      "zircon.autorun.boot=/boot/bin/devicetree-extract";

  auto fdt = crosvm_arm();
  boot_shim::DevicetreeChosenNodeMatcher<AllUartDrivers> chosen_matcher("test", stdout);

  ASSERT_TRUE(devicetree::Match(fdt, chosen_matcher));

  CheckChosenMatcher(chosen_matcher,
                     {
                         .ramdisk_start = 0x81000000,
                         .ramdisk_end = 0x82bd4e28,
                         .cmdline = kCmdline,
                         .uart_config_name = uart::ns8250::Mmio8Driver::kConfigName,
                         .uart_config =
                             {
                                 .mmio_phys = 0x3F8,
                                 .irq = 32,
                                 .flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_EDGE_TRIGGERED |
                                          ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
                             },
                         .uart_absolute_path = "/U6_16550A@3f8",
                     });
}

TEST_F(ChosenNodeMatcherTest, QemuArm) {
  constexpr std::string_view kQemuCmdline =
      "TERM=xterm-256color "
      "kernel.entropy-mixin=cd93b8955fc588b1bcde0d691a694b926d53faeca61c386635739b24df717363 "
      "kernel.halt-on-panic=true ";
  constexpr uint32_t kQemuRamdiskStart = 0x48000000;
  constexpr uint32_t kQemuRamdiskEnd = 0x499e8458;

  auto fdt = qemu_arm_gic3();
  boot_shim::DevicetreeChosenNodeMatcher<AllUartDrivers> chosen_matcher("test", stdout);

  ASSERT_TRUE(devicetree::Match(fdt, chosen_matcher));

  CheckChosenMatcher(chosen_matcher,
                     {
                         .ramdisk_start = kQemuRamdiskStart,
                         .ramdisk_end = kQemuRamdiskEnd,
                         .cmdline = kQemuCmdline,
                         .uart_config_name = uart::pl011::Driver::kConfigName,
                         .uart_config =
                             {
                                 .mmio_phys = uart::pl011::kQemuConfig.mmio_phys,
                                 .irq = 33,
                                 .flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_LEVEL_TRIGGERED |
                                          ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
                             },
                         .uart_absolute_path = "/pl011@9000000",
                     });
}

TEST_F(ChosenNodeMatcherTest, QemuRiscv) {
  constexpr std::string_view kQemuCmdline =
      "BOOT_IMAGE=/vmlinuz-5.19.0-1012-generic root=/dev/mapper/ubuntu--vg-ubuntu--lv ro";
  constexpr uint32_t kQemuRamdiskStart = 0xD646A000;
  constexpr uint32_t kQemuRamdiskEnd = 0xDAFEFDB6;

  auto fdt = qemu_riscv();
  boot_shim::DevicetreeChosenNodeMatcher<AllUartDrivers> chosen_matcher("test", stdout);

  ASSERT_TRUE(devicetree::Match(fdt, chosen_matcher));

  CheckChosenMatcher(chosen_matcher, {
                                         .ramdisk_start = kQemuRamdiskStart,
                                         .ramdisk_end = kQemuRamdiskEnd,
                                         .cmdline = kQemuCmdline,
                                         .uart_config_name = uart::ns8250::Mmio8Driver::kConfigName,
                                         .uart_config =
                                             {
                                                 .mmio_phys = 0x10000000,
                                                 .irq = 10,
                                                 .flags = 0,
                                             },
                                         .uart_absolute_path = "/soc/serial@10000000",
                                     });
}

TEST_F(ChosenNodeMatcherTest, VisionFive2) {
  constexpr std::string_view kCmdline =
      "root=/dev/mmcblk1p4 rw console=tty0 console=ttyS0,115200 earlycon rootwait "
      "stmmaceth=chain_mode:1 selinux=0";

  auto fdt = vision_five_2();
  boot_shim::DevicetreeChosenNodeMatcher<AllUartDrivers> chosen_matcher("test", stdout);

  ASSERT_TRUE(devicetree::Match(fdt, chosen_matcher));

  CheckChosenMatcher(chosen_matcher, {
                                         .ramdisk_start = 0x48100000,
                                         .ramdisk_end = 0x48fb3df5,
                                         .cmdline = kCmdline,
                                         .uart_config_name = uart::dw8250::Driver::kConfigName,
                                         .uart_config =
                                             {
                                                 .mmio_phys = 0x10000000,
                                                 .irq = 32,
                                                 .flags = 0,
                                             },
                                         .uart_absolute_path = "/soc/serial@10000000",
                                     });
}

TEST_F(ChosenNodeMatcherTest, KhadasVim3) {
  constexpr std::string_view kCmdline =
      " androidboot.verifiedbootstate=orange androidboot.dtbo_idx=3   "
      " androidboot.serialno=06ECB1E62CB2  no_console_suspend console=ttyAML0,115200 earlycon"
      " printk.devkmsg=on androidboot.boot_devices=soc/ffe07000.mmc init=/init"
      " firmware_class.path=/vendor/firmware androidboot.hardware=yukawa"
      " androidboot.selinux=permissive";

  auto fdt = khadas_vim3();
  boot_shim::DevicetreeChosenNodeMatcher<AllUartDrivers> chosen_matcher("test", stdout);

  ASSERT_TRUE(devicetree::Match(fdt, chosen_matcher));

  CheckChosenMatcher(chosen_matcher,
                     {
                         .ramdisk_start = 0x7fe4d000,
                         .ramdisk_end = 0x7ffff5d7,
                         .cmdline = kCmdline,
                         .uart_config_name = uart::amlogic::Driver::kConfigName,
                         .uart_config =
                             {
                                 .mmio_phys = 0xff803000,
                                 .irq = 225,
                                 .flags = ZBI_KERNEL_DRIVER_IRQ_FLAGS_EDGE_TRIGGERED |
                                          ZBI_KERNEL_DRIVER_IRQ_FLAGS_POLARITY_HIGH,
                             },
                         .uart_absolute_path = "/soc/bus@ff800000/serial@3000",
                     });
}

TEST_F(ChosenNodeMatcherTest, BananaPiF3) {
  constexpr std::string_view kCmdline =
      "earlycon=sbi earlyprintk console=tty1 console=ttyS0,115200 loglevel=8 clk_ignore_unused swiotlb=65536 "
      "rdinit=/init root=/dev/mmcblk2p6 rootwait rootfstype=ext4";

  auto fdt = banana_pi_f3();
  boot_shim::DevicetreeChosenNodeMatcher<AllUartDrivers> chosen_matcher("test", stdout);

  ASSERT_TRUE(devicetree::Match(fdt, chosen_matcher));

  CheckChosenMatcher(chosen_matcher, {
                                         .ramdisk_start = 0x7685c000,
                                         .ramdisk_end = 0x76ebf76b,
                                         .cmdline = kCmdline,
                                         .uart_config_name = uart::ns8250::PxaDriver::kConfigName,
                                         .uart_config =
                                             {
                                                 .mmio_phys = 0xd4017000,
                                                 .irq = 0x2a,
                                                 .flags = 0,
                                             },
                                         .uart_absolute_path = "/soc/serial@d4017000",
                                     });
}

}  // namespace
