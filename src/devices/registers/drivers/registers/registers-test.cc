// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "registers.h"

#include <fidl/fuchsia.driver.metadata/cpp/fidl.h>
#include <lib/driver/component/cpp/internal/driver_server2.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/mock-mmio/cpp/region.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>

#include "src/devices/lib/fidl-metadata/registers.h"
#include "src/lib/testing/predicates/status.h"

namespace registers {

struct TestMaskEntry {
  uint64_t offset;
  uint32_t size;
  std::optional<uint64_t> mask;
  std::optional<uint32_t> count;
  std::optional<bool> overlap_check;
};

struct TestRegisterEntry {
  std::string name;
  uint32_t mmio_id;
  std::vector<TestMaskEntry> masks;
};

fuchsia_driver_metadata::Dictionary CreateRegistersMetadataDictionary(
    const std::vector<TestRegisterEntry>& registers) {
  std::vector<fuchsia_driver_metadata::DictionaryEntry> entries;

  entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
      "registers._count",
      fuchsia_driver_metadata::DictionaryValue::WithInt64(static_cast<int64_t>(registers.size()))));

  for (size_t i = 0; i < registers.size(); ++i) {
    const auto& reg = registers[i];
    std::string reg_prefix = "registers." + std::to_string(i);

    entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
        reg_prefix + ".name", fuchsia_driver_metadata::DictionaryValue::WithStr(reg.name)));
    entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
        reg_prefix + ".mmio_id", fuchsia_driver_metadata::DictionaryValue::WithInt64(reg.mmio_id)));
    entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
        reg_prefix + ".masks._count", fuchsia_driver_metadata::DictionaryValue::WithInt64(
                                          static_cast<int64_t>(reg.masks.size()))));

    for (size_t j = 0; j < reg.masks.size(); ++j) {
      const auto& mask = reg.masks[j];
      std::string mask_prefix = reg_prefix + ".masks." + std::to_string(j);

      entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
          mask_prefix + ".offset",
          fuchsia_driver_metadata::DictionaryValue::WithInt64(mask.offset)));
      entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
          mask_prefix + ".size", fuchsia_driver_metadata::DictionaryValue::WithInt64(mask.size)));
      if (mask.mask) {
        entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
            mask_prefix + ".mask",
            fuchsia_driver_metadata::DictionaryValue::WithInt64(static_cast<int64_t>(*mask.mask))));
      }
      if (mask.count) {
        entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
            mask_prefix + ".count",
            fuchsia_driver_metadata::DictionaryValue::WithInt64(*mask.count)));
      }
      if (mask.overlap_check) {
        entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
            mask_prefix + ".overlap_check",
            fuchsia_driver_metadata::DictionaryValue::WithBoolean(*mask.overlap_check)));
      }
    }
  }

  fuchsia_driver_metadata::Dictionary dict;
  dict.entries(std::move(entries));
  return dict;
}

namespace {

constexpr uint32_t kRegCount = 3;
constexpr size_t kRegSize = 0x00000100;

}  // namespace

class TestRegistersDevice : public RegistersDevice {
 public:
  TestRegistersDevice() = default;
  ~TestRegistersDevice() override {
    for (const auto& i : mock_mmio_) {
      if (i) {
        i->VerifyAll();
      }
    }
  }

  static DriverRegistration GetDriverRegistration() {
    return FUCHSIA_DRIVER_REGISTRATION_V1(
        ::fdf_internal::DriverServer2<TestRegistersDevice>::initialize,
        ::fdf_internal::DriverServer2<TestRegistersDevice>::destroy);
  }

  zx::result<> MapMmio(fuchsia_hardware_registers::Mask::Tag& tag) override {
    std::map<uint32_t, std::shared_ptr<MmioInfo>> mmios;
    for (uint32_t i = 0; i < kRegCount; i++) {
      mock_mmio_[i] = std::make_unique<mock_mmio::Region>(SWITCH_BY_TAG(tag, GetSize),
                                                          kRegSize / SWITCH_BY_TAG(tag, GetSize));

      zx::result<MmioInfo> mmio_info =
          SWITCH_BY_TAG(tag, MmioInfo::Create, mock_mmio_[i]->GetMmioBuffer());
      EXPECT_OK(mmio_info);
      mmios_.emplace(i, std::make_shared<MmioInfo>(std::move(*mmio_info)));
    }

    return zx::ok();
  }

  std::array<std::unique_ptr<mock_mmio::Region>, kRegCount> mock_mmio_;
};

class RegistersDeviceTestEnvironment : fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    zx::result result = to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
        pdev_.GetInstanceHandler(fdf::Dispatcher::GetCurrent()->async_dispatcher()), "pdev");
    if (result.is_error()) {
      return result.take_error();
    }

    return zx::ok();
  }

  template <typename T>
  void Init(std::span<const fidl_metadata::registers::Register<T>> kRegisters) {
    zx::result metadata = fidl_metadata::registers::RegistersMetadataToFidl(kRegisters);
    ASSERT_OK(metadata);
    ASSERT_OK(pdev_.AddFidlMetadata(fuchsia_hardware_registers::Metadata::kSerializableName,
                                    metadata.value()));
  }

  void Init(fuchsia_driver_metadata::Dictionary metadata) {
    ASSERT_OK(pdev_.AddFidlMetadata("fuchsia.hardware.registers.Metadata", metadata));
  }

 private:
  fdf_fake::FakePDev pdev_;
};

class RegistersDeviceTestConfig final {
 public:
  using DriverType = TestRegistersDevice;
  using EnvironmentType = RegistersDeviceTestEnvironment;
};

class RegistersDeviceTest : public ::testing::Test {
 public:
  template <typename T>
  zx::result<> TryInit(std::span<const fidl_metadata::registers::Register<T>> registers) {
    driver_test().RunInEnvironmentTypeContext(
        [registers](RegistersDeviceTestEnvironment& env) { env.Init(registers); });
    return driver_test().StartDriver();
  }

  template <typename T>
  void Init(std::span<const fidl_metadata::registers::Register<T>> registers) {
    ASSERT_OK(TryInit(registers));
  }

  zx::result<> TryInit(fuchsia_driver_metadata::Dictionary metadata) {
    driver_test().RunInEnvironmentTypeContext(
        [metadata = std::move(metadata)](RegistersDeviceTestEnvironment& env) mutable {
          env.Init(std::move(metadata));
        });
    return driver_test().StartDriver();
  }

  void Init(fuchsia_driver_metadata::Dictionary metadata) {
    ASSERT_OK(TryInit(std::move(metadata)));
  }

  fidl::ClientEnd<fuchsia_hardware_registers::Device> GetRegisterClient(std::string_view reg_id) {
    zx::result result = driver_test().Connect<fuchsia_hardware_registers::Service::Device>(reg_id);
    EXPECT_OK(result.status_value());
    return std::move(result.value());
  }

  void TearDown() override { ASSERT_OK(driver_test().StopDriver()); }

  fdf_testing::BackgroundDriverTest<RegistersDeviceTestConfig>& driver_test() {
    return driver_test_;
  }

 private:
  fdf_testing::BackgroundDriverTest<RegistersDeviceTestConfig> driver_test_;
};

TEST_F(RegistersDeviceTest, OverlappingBitsTestFailure) {
  ASSERT_TRUE(TryInit<uint32_t>(std::vector<fidl_metadata::registers::Register<uint32_t>>{
                                    {
                                        .name = "test0",
                                        .mmio_id = 0,
                                        .masks =
                                            {
                                                {
                                                    .value = 0xFFFFFFFF,
                                                    .mmio_offset = 0,
                                                },
                                                {
                                                    .value = 0xFFFFFFFF,
                                                    .mmio_offset = 0,
                                                },
                                            },
                                    },
                                })
                  .is_error());
}

TEST_F(RegistersDeviceTest, OverlappingBitsInTailFailure) {
  ASSERT_TRUE(TryInit<uint32_t>(std::vector<fidl_metadata::registers::Register<uint32_t>>{
                                    {
                                        .name = "test0",
                                        .mmio_id = 0,
                                        .masks =
                                            {
                                                {
                                                    .value = 0xFFFFFFFF,
                                                    .mmio_offset = 0,
                                                    .count = 2,
                                                },
                                                {
                                                    .value = 0xFFFFFFFF,
                                                    .mmio_offset = 4,
                                                },
                                            },
                                    },
                                })
                  .is_error());
}

TEST_F(RegistersDeviceTest, Read32Test) {
  Init<uint32_t>(std::vector<fidl_metadata::registers::Register<uint32_t>>{
      {
          .name = "test0",
          .mmio_id = 0,
          .masks =
              {
                  {
                      .value = 0xFFFFFFFF,
                      .mmio_offset = 0,
                  },
              },
      },
      {
          .name = "test1",
          .mmio_id = 2,
          .masks =
              {
                  {
                      .value = 0xFFFFFFFF,
                      .mmio_offset = 0,
                      .count = 2,
                  },
                  {
                      .value = 0xFFFF0000,
                      .mmio_offset = 0x8,
                  },
              },
      },
  });

  auto test0 = fidl::WireSyncClient<fuchsia_hardware_registers::Device>(GetRegisterClient("test0"));
  auto test1 = fidl::WireSyncClient<fuchsia_hardware_registers::Device>(GetRegisterClient("test1"));

  // Invalid Call
  {
    auto result = test0->ReadRegister8(/* offset: */ 0x0, /* mask: */ 0xFF);
    EXPECT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Address not aligned
  {
    auto result = test0->ReadRegister32(/* offset: */ 0x1, /* mask: */ 0xFFFFFFFF);
    EXPECT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Address out of range
  {
    auto result = test1->ReadRegister32(/* offset: */ 0xC, /* mask: */ 0xFFFFFFFF);
    EXPECT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Invalid mask
  {
    auto result = test1->ReadRegister32(/* offset: */ 0x8, /* mask: */ 0xFFFFFFFF);
    EXPECT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Successful
  driver_test().RunInDriverContext(
      [](TestRegistersDevice& driver) { (*(driver.mock_mmio_[0]))[0x0].ExpectRead(0x12341234); });
  {
    auto result = test0->ReadRegister32(/* offset: */ 0x0, /* mask: */ 0xFFFFFFFF);
    EXPECT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->value, 0x12341234U);
  };

  driver_test().RunInDriverContext(
      [](TestRegistersDevice& driver) { (*(driver.mock_mmio_[2]))[0x4].ExpectRead(0x12341234); });
  {
    auto result = test1->ReadRegister32(/* offset: */ 0x4, /* mask: */ 0xFFFF0000);
    EXPECT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->value, 0x12340000U);
  };

  driver_test().RunInDriverContext(
      [](TestRegistersDevice& driver) { (*(driver.mock_mmio_[2]))[0x8].ExpectRead(0x12341234); });
  {
    auto result = test1->ReadRegister32(/* offset: */ 0x8, /* mask: */ 0xFFFF0000);
    EXPECT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->value, 0x12340000U);
  };
}

TEST_F(RegistersDeviceTest, Write32Test) {
  Init<uint32_t>(std::vector<fidl_metadata::registers::Register<uint32_t>>{
      {
          .name = "test0",
          .mmio_id = 0,
          .masks =
              {
                  {
                      .value = 0xFFFFFFFF,
                      .mmio_offset = 0,
                  },
              },
      },
      {
          .name = "test1",
          .mmio_id = 1,
          .masks =
              {
                  {
                      .value = 0xFFFFFFFF,
                      .mmio_offset = 0,
                      .count = 2,
                  },
                  {
                      .value = 0xFFFF0000,
                      .mmio_offset = 0x8,
                  },
              },
      },
  });

  auto test0 = fidl::WireSyncClient<fuchsia_hardware_registers::Device>(GetRegisterClient("test0"));
  auto test1 = fidl::WireSyncClient<fuchsia_hardware_registers::Device>(GetRegisterClient("test1"));

  // Invalid Call
  {
    auto result = test0->WriteRegister8(/* offset: */ 0x0, /* mask: */ 0xFF, /* value:  */ 0x12);
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Address not aligned
  {
    auto result = test0->WriteRegister32(
        /* offset: */ 0x1, /* mask: */ 0xFFFFFFFF, /* value: */ 0x43214321);
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Address out of range
  {
    auto result = test1->WriteRegister32(
        /* offset: */ 0xC, /* mask: */ 0xFFFFFFFF, /* value: */ 0x43214321);
    EXPECT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Invalid mask
  {
    auto result = test1->WriteRegister32(
        /* offset: */ 0x8, /* mask: */ 0xFFFFFFFF, /* value: */ 0x43214321);
    EXPECT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Successful
  driver_test().RunInDriverContext([](TestRegistersDevice& driver) {
    (*(driver.mock_mmio_[0]))[0x0].ExpectRead(0x00000000).ExpectWrite(0x43214321);
  });
  {
    auto result = test0->WriteRegister32(
        /* offset: */ 0x0, /* mask: */ 0xFFFFFFFF, /* value: */
        0x43214321);
    EXPECT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  };

  driver_test().RunInDriverContext([](TestRegistersDevice& driver) {
    (*(driver.mock_mmio_[1]))[0x4].ExpectRead(0x00000000).ExpectWrite(0x43210000);
  });
  {
    auto result = test1->WriteRegister32(
        /* offset: */ 0x4, /* mask: */ 0xFFFF0000, /* value: */
        0x43214321);
    EXPECT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  };

  driver_test().RunInDriverContext([](TestRegistersDevice& driver) {
    (*(driver.mock_mmio_[1]))[0x8].ExpectRead(0x00000000).ExpectWrite(0x43210000);
  });
  {
    auto result = test1->WriteRegister32(
        /* offset: */ 0x8, /* mask: */ 0xFFFF0000, /* value: */
        0x43214321);
    EXPECT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  };
}

TEST_F(RegistersDeviceTest, Read64Test) {
  Init<uint64_t>(std::vector<fidl_metadata::registers::Register<uint64_t>>{
      {
          .name = "test0",
          .mmio_id = 0,
          .masks =
              {
                  {
                      .value = 0xFFFFFFFFFFFFFFFF,
                      .mmio_offset = 0,
                  },
              },
      },
      {
          .name = "test1",
          .mmio_id = 2,
          .masks =
              {
                  {
                      .value = 0xFFFFFFFFFFFFFFFF,
                      .mmio_offset = 0,
                  },
                  {
                      .value = 0x00000000FFFFFFFF,
                      .mmio_offset = 0x8,
                  },
                  {
                      .value = 0xFFFFFFFF00000000,
                      .mmio_offset = 0x10,
                  },
              },
      },
  });

  auto test0 = fidl::WireSyncClient<fuchsia_hardware_registers::Device>(GetRegisterClient("test0"));
  auto test1 = fidl::WireSyncClient<fuchsia_hardware_registers::Device>(GetRegisterClient("test1"));

  // Invalid Call
  {
    auto result = test0->ReadRegister8(/* offset: */ 0x0, /* mask: */ 0xFF);
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Address not aligned
  {
    auto result = test0->ReadRegister64(/* offset: */ 0x1, /* mask: */
                                        0xFFFFFFFFFFFFFFFF);
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Address out of range
  {
    auto result = test1->ReadRegister64(/* offset: */ 0x20, /* mask: */ 0xFFFFFFFFFFFFFFFF);
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Invalid mask
  {
    auto result = test1->ReadRegister64(/* offset: */ 0x8, /* mask: */ 0xFFFFFFFFFFFFFFFF);
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Successful
  driver_test().RunInDriverContext([](TestRegistersDevice& driver) {
    (*(driver.mock_mmio_[0]))[0x0].ExpectRead(0x1234123412341234);
  });
  {
    auto result = test0->ReadRegister64(/* offset: */ 0x0, /* mask: */ 0xFFFFFFFFFFFFFFFF);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->value, 0x1234123412341234UL);
  };

  driver_test().RunInDriverContext([](TestRegistersDevice& driver) {
    (*(driver.mock_mmio_[2]))[0x8].ExpectRead(0x1234123412341234);
  });
  {
    auto result = test1->ReadRegister64(/* offset: */ 0x8, /* mask: */ 0x00000000FFFF0000);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->value, 0x0000000012340000UL);
  };
}

TEST_F(RegistersDeviceTest, Write64Test) {
  Init<uint64_t>(std::vector<fidl_metadata::registers::Register<uint64_t>>{
      {
          .name = "test0",
          .mmio_id = 0,
          .masks =
              {
                  {
                      .value = 0xFFFFFFFFFFFFFFFF,
                      .mmio_offset = 0,
                  },
              },
      },
      {
          .name = "test1",
          .mmio_id = 1,
          .masks =
              {
                  {
                      .value = 0xFFFFFFFFFFFFFFFF,
                      .mmio_offset = 0,
                  },
                  {
                      .value = 0x00000000FFFFFFFF,
                      .mmio_offset = 0x8,
                  },
                  {
                      .value = 0xFFFFFFFF00000000,
                      .mmio_offset = 0x10,
                  },
              },
      },
  });

  auto test0 = fidl::WireSyncClient<fuchsia_hardware_registers::Device>(GetRegisterClient("test0"));
  auto test1 = fidl::WireSyncClient<fuchsia_hardware_registers::Device>(GetRegisterClient("test1"));

  // Invalid Call
  {
    auto result = test0->WriteRegister8(/* offset: */ 0x0, /* mask: */ 0xFF, /* value:  */ 0x12);
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Address not aligned
  {
    auto result = test0->WriteRegister64(
        /* offset: */ 0x1, /* mask: */ 0xFFFFFFFFFFFFFFFF, /* value: */
        0x4321432143214321);
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Address out of range
  {
    auto result = test1->WriteRegister64(
        /* offset: */ 0x20, /* mask: */ 0xFFFFFFFFFFFFFFFF, /* value: */
        0x4321432143214321);
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Invalid mask
  {
    auto result = test1->WriteRegister64(/* offset: */ 0x8,
                                         /* mask: */ 0xFFFFFFFFFFFFFFFF,
                                         /* value: */ 0x4321432143214321);
    ASSERT_TRUE(result.ok());
    EXPECT_FALSE(result->is_ok());
  };

  // Successful
  driver_test().RunInDriverContext([](TestRegistersDevice& driver) {
    (*(driver.mock_mmio_[0]))[0x0].ExpectRead(0x0000000000000000).ExpectWrite(0x4321432143214321);
  });
  {
    auto result = test0->WriteRegister64(
        /* offset: */ 0x0, /* mask: */ 0xFFFFFFFFFFFFFFFF, /* value: */
        0x4321432143214321);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  };

  driver_test().RunInDriverContext([](TestRegistersDevice& driver) {
    (*(driver.mock_mmio_[1]))[0x8].ExpectRead(0x0000000000000000).ExpectWrite(0x0000000043210000);
  });
  {
    auto result = test1->WriteRegister64(
        /* offset: */ 0x8, /* mask: */ 0x00000000FFFF0000, /* value: */
        0x0000000043210000);
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
  };
}

TEST_F(RegistersDeviceTest, GenericMetadataSuccess) {
  auto dict = CreateRegistersMetadataDictionary({
      {
          .name = "test0",
          .mmio_id = 0,
          .masks =
              {
                  {
                      .offset = 0,
                      .size = 4,
                      .mask = 0xFFFFFFFF,
                  },
              },
      },
  });

  Init(std::move(dict));

  auto test0 = fidl::WireSyncClient<fuchsia_hardware_registers::Device>(GetRegisterClient("test0"));

  driver_test().RunInDriverContext(
      [](TestRegistersDevice& driver) { (*(driver.mock_mmio_[0]))[0x0].ExpectRead(0x12341234); });
  {
    auto result = test0->ReadRegister32(/* offset: */ 0x0, /* mask: */ 0xFFFFFFFF);
    EXPECT_TRUE(result.ok());
    EXPECT_TRUE(result->is_ok());
    EXPECT_EQ(result->value()->value, 0x12341234U);
  };
}

TEST_F(RegistersDeviceTest, GenericMetadataUnsupportedSize) {
  auto dict = CreateRegistersMetadataDictionary({
      {
          .name = "test0",
          .mmio_id = 0,
          .masks =
              {
                  {
                      .offset = 0,
                      .size = 3,  // Unsupported size
                      .mask = 0xFFFFFFFF,
                  },
              },
      },
  });

  ASSERT_TRUE(TryInit(std::move(dict)).is_error());
}

TEST_F(RegistersDeviceTest, GenericMetadataEmptyRegisters) {
  auto dict = CreateRegistersMetadataDictionary({});
  ASSERT_TRUE(TryInit(std::move(dict)).is_error());
}

TEST_F(RegistersDeviceTest, GenericMetadataRegisterWithNoMasks) {
  auto dict = CreateRegistersMetadataDictionary({
      {
          .name = "test0",
          .mmio_id = 0,
          .masks = {},
      },
  });
  ASSERT_TRUE(TryInit(std::move(dict)).is_error());
}

TEST_F(RegistersDeviceTest, GenericMetadataMaskWithNoValue) {
  auto dict = CreateRegistersMetadataDictionary({
      {
          .name = "test0",
          .mmio_id = 0,
          .masks =
              {
                  {
                      .offset = 0,
                      .size = 4,
                      .mask = std::nullopt,
                  },
              },
      },
  });
  ASSERT_TRUE(TryInit(std::move(dict)).is_error());
}

}  // namespace registers
