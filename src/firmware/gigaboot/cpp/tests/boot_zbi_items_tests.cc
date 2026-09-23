// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/abr/abr.h>
#include <lib/zbi-format/driver-config.h>
#include <lib/zbi-format/graphics.h>
#include <lib/zbi-format/memory.h>
#include <lib/zbi-format/zbi.h>
#include <zircon/limits.h>

#include <array>
#include <numeric>

#include <efi/boot-services.h>
#include <efi/protocol/graphics-output.h>
#include <efi/types.h>
#include <gtest/gtest.h>

#include "acpi.h"
#include "boot_zbi_items.h"
#include "mock_boot_service.h"
#include "page_size.h"
#include "utils.h"

extern "C" efi_status generate_efi_memory_attributes_table_item(
    void *ramdisk, const size_t ramdisk_size, efi_system_table *sys, const void *mmap,
    size_t memory_map_size, size_t dsize) {
  return EFI_SUCCESS;
}

namespace gigaboot {
namespace {

uint8_t CalculateChecksum(std::span<const uint8_t> bytes) {
  // Add an explicit init of 64 bit 0 so that the sum doesn't overflow.
  int64_t intermediate = std::reduce(bytes.begin(), bytes.end(), 0ll);
  return static_cast<uint8_t>(0x100 - (intermediate & 0xFF));
}

class BootZbiItemTest : public ::testing::Test {
 public:
  BootZbiItemTest() : image_device_({"path-A", "path-B", "path-C", "image"}) {
    stub_service_.AddDevice(&image_device_);
  }

  auto SetupEfiGlobalState(EfiConfigTable const &config_table = *kDefaultEfiConfigTable) {
    return gigaboot::SetupEfiGlobalState(stub_service_, image_device_, config_table);
  }

  MockStubService &stub_service() { return stub_service_; }
  ZbiContext &context() { return zbi_context_; }

  std::span<uint8_t> buffer() { return buffer_; }

 private:
  MockStubService stub_service_;
  Device image_device_;
  std::array<uint8_t, 1024> buffer_ = {};
  ZbiContext zbi_context_;
};

class AcpiTableTest : public BootZbiItemTest {
 public:
  // ACPI tables live in memory as packed, adjacent structures, and in many cases define
  // ad-hoc arrays of related structures.
  // The normal way of dealing with this in implementation code is to just cast pointers and assume.
  // However dangerous and awful this is, it's the current situation.
  //
  // Test setup code needs to maintain these expectations, and so that requires making
  // relevant structures live next to each other in memory.
  // The root SDT table is followed by an array of 32 or 64 bit integers (depending on revision)
  // that are actually raw pointers to other SDT child structures.
  // For the sake of testing, just assert that all pointers are 64 bits, which means
  // we only test rev 2 RSDP.
  struct __attribute__((packed)) SdtHolder {
    static_assert(sizeof(void *) == sizeof(uint64_t), "Test assumes 64 bit pointers");

    SdtHolder()
        : sdt_table{
              .signature = kXsdtSignature,
              .length = static_cast<uint32_t>(sizeof(sdt_table) + sizeof(extra_tables)),
          } {}

    // Add an SDT table to the pointer array following the primary table.
    // This indirection is necessary to avoid undefined behavior due to alignment requirements.
    void InsertSdtTable(size_t index, const void *table) {
      ASSERT_LT(index, extra_tables.size() / sizeof(void *));
      memcpy(extra_tables.data() + index * sizeof(void *), &table, sizeof(table));
    }

    SdtHeader sdt_table;
    std::array<uint8_t, 4 * sizeof(void *)> extra_tables = {};
  };

  AcpiTableTest() : config_table_(2) {
    AcpiRsdp &rsdp = config_table_.rsdp();
    rsdp = {
        .signature = kAcpiRsdpSignature,
        .checksum = 0,
        .revision = 1,  // Actually rev 2
        .length = sizeof(rsdp),
        // For rev 2 and onward, the SDT address lives in the xsdt_address field and is 64 bits.
        // For rev 1, the address would live in rsdt_address and would be 32 bits.
        .xsdt_address = reinterpret_cast<uint64_t>(&sdt_holder_.sdt_table),
        .extended_checksum = 0,
    };
    std::span<const uint8_t> acpi_bytes(reinterpret_cast<const uint8_t *>(&rsdp), kAcpiRsdpV1Size);
    rsdp.checksum = CalculateChecksum(acpi_bytes);

    acpi_bytes = {reinterpret_cast<const uint8_t *>(&rsdp), rsdp.length};
    rsdp.extended_checksum = CalculateChecksum(acpi_bytes);
  }

  SdtHolder &sdt_holder() { return sdt_holder_; }
  const EfiConfigTable &config_table() const { return config_table_; }
  EfiConfigTable &config_table() { return config_table_; }

 private:
  EfiConfigTable config_table_;
  SdtHolder sdt_holder_;
};

TEST_F(BootZbiItemTest, AddMemoryItems) {
  auto cleanup = SetupEfiGlobalState();

  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);

  // Don't care actual values. Choose any for test purpose.
  std::vector<efi_memory_descriptor> memory_map = {
      {
          .Type = EfiReservedMemoryType,
          .Padding = 0,
          .PhysicalStart = 0x0,
          .VirtualStart = 0x100000,
          .NumberOfPages = 0x10,
          .Attribute = EFI_MEMORY_UC,
      },
      {
          .Type = EfiLoaderCode,
          .Padding = 0,
          .PhysicalStart = 0x1000,
          .VirtualStart = 0x200000,
          .NumberOfPages = 0x10,
          .Attribute = EFI_MEMORY_UC,
      },
  };

  context().uart_mmio_phys = 16;
  context().num_cpu_nodes = 2;
  context().gic_driver = zbi_dcfg_arm_gic_v3_driver_t{
      .mmio_phys = 0x100,
      .gicd_offset = 0x200,
      .gicr_offset = 0x300,
      .gicr_stride = 0x400,
  };

  const size_t kMkey = 123;
  stub_service().SetMemoryMap(memory_map, kMkey);
  auto res = AddMemoryItems(reinterpret_cast<zbi_header_t *>(buffer().data()), buffer().size(),
                            &context());
  ASSERT_TRUE(res.is_ok());
  ASSERT_EQ(res.value(), kMkey);

  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_MEM_CONFIG);
  ASSERT_EQ(items.size(), 1ULL);

  std::span<const zbi_mem_range_t> zbi_mem_ranges = {
      reinterpret_cast<const zbi_mem_range_t *>(items[0].data()),
      items[0].size() / sizeof(zbi_mem_range_t)};
  ASSERT_EQ(zbi_mem_ranges.size(), 4ULL);

  // Make sure that we added the expected items.
  EXPECT_EQ(zbi_mem_ranges[0].paddr, 0x0ULL);
  EXPECT_EQ(zbi_mem_ranges[0].length, 0x10 * PAGE_SIZE);
  EXPECT_EQ(zbi_mem_ranges[0].type, EfiToZbiMemRangeType(EfiReservedMemoryType));

  EXPECT_EQ(zbi_mem_ranges[1].paddr, 0x1000ULL);
  EXPECT_EQ(zbi_mem_ranges[1].length, 0x10 * PAGE_SIZE);
  EXPECT_EQ(zbi_mem_ranges[1].type, EfiToZbiMemRangeType(EfiLoaderCode));
  EXPECT_EQ(zbi_mem_ranges[2].type, ZBI_MEM_TYPE_PERIPHERAL);
  EXPECT_EQ(zbi_mem_ranges[3].type, ZBI_MEM_TYPE_PERIPHERAL);
}

TEST_F(BootZbiItemTest, AppendAbrSlotA) {
  auto cleanup = SetupEfiGlobalState();

  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_TRUE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                  buffer().size(), &slot, &context()));

  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_CMDLINE);
  EXPECT_EQ(items.size(), 1ULL);

  ASSERT_EQ(std::string_view(reinterpret_cast<const char *>(items[0].data())),
            "zvb.current_slot=_a");
}

TEST_F(BootZbiItemTest, AppendAbrSlotB) {
  auto cleanup = SetupEfiGlobalState();

  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexB;
  ASSERT_TRUE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                  buffer().size(), &slot, &context()));

  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_CMDLINE);
  EXPECT_EQ(items.size(), 1ULL);

  ASSERT_EQ(std::string_view(reinterpret_cast<const char *>(items[0].data())),
            "zvb.current_slot=_b");
}

TEST_F(BootZbiItemTest, AcpiRsdpTestV2) {
  EfiConfigTable config_table(2);
  auto cleanup = SetupEfiGlobalState(config_table);

  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_TRUE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                  buffer().size(), &slot, &context()));

  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_ACPI_RSDP);
  ASSERT_EQ(items.size(), 1ULL);

  ASSERT_TRUE(memcmp(*reinterpret_cast<void *const *>(items[0].data()), &config_table.rsdp(),
                     sizeof(config_table.rsdp())) == 0);
}

TEST_F(BootZbiItemTest, AcpiRsdpV1) {
  EfiConfigTable config_table(1);
  auto cleanup = SetupEfiGlobalState(config_table);

  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_TRUE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                  buffer().size(), &slot, &context()));

  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_ACPI_RSDP);
  ASSERT_EQ(items.size(), 1ULL);

  ASSERT_TRUE(memcmp(*reinterpret_cast<AcpiRsdp *const *>(items[0].data()), &config_table.rsdp(),
                     sizeof(config_table.rsdp())) == 0);
}

TEST_F(BootZbiItemTest, AcpiRsdpV1CorruptTest) {
  EfiConfigTable config_table(1);
  config_table.CorruptChecksum();
  auto cleanup = SetupEfiGlobalState(config_table);

  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_FALSE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                   buffer().size(), &slot, &context()));
}
TEST_F(BootZbiItemTest, AcpiRsdpV2CorruptTest) {
  EfiConfigTable config_table(1);
  config_table.CorruptV2Checksum();
  auto cleanup = SetupEfiGlobalState(config_table);

  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_FALSE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                   buffer().size(), &slot, &context()));
}
TEST_F(BootZbiItemTest, AcpiRsdpNotFoundTest) {
  EfiConfigTable config_table(1);
  config_table.CorruptSignature();
  auto cleanup = SetupEfiGlobalState(config_table);

  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_FALSE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                   buffer().size(), &slot, &context()));
}

TEST_F(AcpiTableTest, AcpiUartDriver) {
  AcpiSpcr spcr = {
      .hdr = {.signature = AcpiSpcr::kSig, .revision = 2},
      .interface_type = 0x0003,
      .base_address = {.address = 0xDEADBEEFCABBA6E5},
      .interrupt_type = 0x0,
      .gsiv = 0xCAFED00D,
  };
  sdt_holder().InsertSdtTable(0, &spcr);

  auto cleanup = SetupEfiGlobalState(config_table());

  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_TRUE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                  buffer().size(), &slot, &context()));
  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_KERNEL_DRIVER);
  ASSERT_EQ(items.size(), 1ULL);
}

TEST_F(AcpiTableTest, AcpiPsciDriver) {
  AcpiFadt fadt = {
      .hdr = {.signature = AcpiFadt::kSig, .revision = 2},
      .arm_boot_arch = 0b11,
  };
  sdt_holder().InsertSdtTable(0, &fadt);

  auto cleanup = SetupEfiGlobalState(config_table());
  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_TRUE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                  buffer().size(), &slot, &context()));
  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_KERNEL_DRIVER);
  ASSERT_EQ(items.size(), 1ULL);
}

TEST_F(AcpiTableTest, AcpiArmTimerDriver) {
  AcpiGtdt gtdt = {
      .hdr = {.signature = AcpiGtdt::kSig, .revision = 2},
  };
  sdt_holder().InsertSdtTable(0, &gtdt);

  auto cleanup = SetupEfiGlobalState(config_table());
  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_TRUE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                  buffer().size(), &slot, &context()));
  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_KERNEL_DRIVER);
  ASSERT_EQ(items.size(), 1ULL);
}

TEST_F(AcpiTableTest, NoSdtTable) {
  AcpiGtdt gtdt = {
      .hdr = {.signature = AcpiGtdt::kSig, .revision = 2},
  };
  sdt_holder().InsertSdtTable(0, &gtdt);

  AcpiRsdp &rsdp = config_table().rsdp();
  rsdp.xsdt_address = 0;
  rsdp.extended_checksum = 0;
  std::span<const uint8_t> rsdp_bytes = {reinterpret_cast<const uint8_t *>(&rsdp), rsdp.length};
  rsdp.extended_checksum = CalculateChecksum(rsdp_bytes);

  auto cleanup = SetupEfiGlobalState(config_table());
  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_TRUE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                  buffer().size(), &slot, &context()));
  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_KERNEL_DRIVER);
  ASSERT_TRUE(items.empty());
}

TEST_F(AcpiTableTest, BadSDTSignature) {
  AcpiGtdt gtdt = {
      .hdr = {.signature = AcpiGtdt::kSig, .revision = 2},
  };
  sdt_holder().InsertSdtTable(0, &gtdt);
  sdt_holder().sdt_table.signature[0]++;

  auto cleanup = SetupEfiGlobalState(config_table());
  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_TRUE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                  buffer().size(), &slot, &context()));
  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_KERNEL_DRIVER);
  ASSERT_TRUE(items.empty());
}

TEST_F(AcpiTableTest, MadtItems) {
  struct __attribute__((packed)) {
    AcpiMadt madt = {.hdr = {.signature = AcpiMadt::kSig, .revision = 2}};
    AcpiMadtGicInterface interrupt_controller = {
        .hdr = {.type = AcpiMadtGicInterface::kType, .length = sizeof(AcpiMadtGicInterface)},
        .cpu_interface_number = 4,
        .mpidr = 0xABCDEF01,
    };
    AcpiMadtGicDistributor distributor = {
        .hdr = {.type = AcpiMadtGicDistributor::kType, .length = sizeof(AcpiMadtGicDistributor)},
        .gic_version = 0x03,
    };
    AcpiMadtGicRedistributor redistributor = {
        .hdr = {.type = AcpiMadtGicRedistributor::kType,
                .length = sizeof(AcpiMadtGicRedistributor)},
    };
  } madt_and_controllers = {};
  madt_and_controllers.madt.hdr.length = static_cast<uint32_t>(sizeof(madt_and_controllers));

  sdt_holder().InsertSdtTable(0, &madt_and_controllers.madt);

  auto cleanup = SetupEfiGlobalState(config_table());
  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_TRUE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                  buffer().size(), &slot, &context()));
  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_CPU_TOPOLOGY);
  ASSERT_EQ(items.size(), 1ULL);

  items = FindItems(buffer().data(), ZBI_TYPE_KERNEL_DRIVER);
  ASSERT_EQ(items.size(), 1ULL);
}

TEST_F(BootZbiItemTest, PlatformIdTest) {
  auto cleanup = SetupEfiGlobalState();

  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_TRUE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                  buffer().size(), &slot, &context()));

  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_PLATFORM_ID);
  ASSERT_EQ(items.size(), 1ULL);
}

TEST_F(BootZbiItemTest, SmbiosTest) {
  EfiConfigTable config_table(EfiConfigTable::SmbiosRev::kV1);
  auto cleanup = SetupEfiGlobalState(config_table);

  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_TRUE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                  buffer().size(), &slot, &context()));

  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_SMBIOS);
  ASSERT_EQ(items.size(), 1ULL);

  ASSERT_TRUE(memcmp(*reinterpret_cast<uint8_t const *const *>(items[0].data()), "_SM_", 4) == 0);
}

TEST_F(BootZbiItemTest, SmbiosV3Test) {
  EfiConfigTable config_table(EfiConfigTable::SmbiosRev::kV3);
  auto cleanup = SetupEfiGlobalState(config_table);

  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_TRUE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                  buffer().size(), &slot, &context()));

  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_SMBIOS);
  ASSERT_EQ(items.size(), 1ULL);

  ASSERT_TRUE(memcmp(*reinterpret_cast<uint8_t const *const *>(items[0].data()), "_SM3_", 5) == 0);
}

TEST_F(BootZbiItemTest, SmbiosErrorTest) {
  EfiConfigTable config_table(EfiConfigTable::SmbiosRev::kNone);
  auto cleanup = SetupEfiGlobalState(config_table);

  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_FALSE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                   buffer().size(), &slot, &context()));
}

struct PixelFormatTestCase {
  char const *test_name;
  efi_graphics_pixel_format format = PixelBitMask;
  efi_pixel_bitmask mask = {};
  uint32_t expected_format;
};

class PixelFormatTest : public BootZbiItemTest,
                        public testing::WithParamInterface<PixelFormatTestCase> {};

TEST_P(PixelFormatTest, TestPixelFormat) {
  PixelFormatTestCase const &test_case = GetParam();
  auto cleanup = SetupEfiGlobalState();
  GraphicsOutputDevice gd;
  gd.mode().Info->PixelFormat = test_case.format;
  gd.mode().Info->PixelInformation = test_case.mask;
  gd.mode().FrameBufferBase = 0xDEADBEEF;
  gd.mode().Info->HorizontalResolution = 1024;
  gd.mode().Info->VerticalResolution = 768;
  gd.mode().Info->PixelsPerScanLine = 15;
  stub_service().AddDevice(&gd);

  zbi_swfb_t expected_framebuffer = {
      .base = 0xDEADBEEF,
      .width = 1024,
      .height = 768,
      .stride = 15,
      .format = test_case.expected_format,
  };

  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_TRUE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                  buffer().size(), &slot, &context()));

  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_FRAMEBUFFER);
  ASSERT_EQ(items.size(), 1ULL);

  ASSERT_TRUE(memcmp(items[0].data(), &expected_framebuffer, sizeof(expected_framebuffer)) == 0);
}

INSTANTIATE_TEST_SUITE_P(
    PixelFormatTests, PixelFormatTest,
    testing::ValuesIn<PixelFormatTest::ParamType>({
        {
            .test_name = "RGB_x888",
            .mask = {.RedMask = 0xFF0000, .GreenMask = 0xFF00, .BlueMask = 0xFF},
            .expected_format = ZBI_PIXEL_FORMAT_RGB_X888,
        },
        {
            .test_name = "RGB_332",
            .mask = {.RedMask = 0xE0, .GreenMask = 0x1C, .BlueMask = 0x3},
            .expected_format = ZBI_PIXEL_FORMAT_RGB_332,
        },
        {
            .test_name = "RGB_565",
            .mask = {.RedMask = 0xF800, .GreenMask = 0x7E0, .BlueMask = 0x1F},
            .expected_format = ZBI_PIXEL_FORMAT_RGB_565,
        },
        {
            .test_name = "RGB_2220",
            .mask = {.RedMask = 0xC0, .GreenMask = 0x30, .BlueMask = 0xC},
            .expected_format = ZBI_PIXEL_FORMAT_RGB_2220,
        },
        {
            .test_name = "unsupported",
            .mask = {.RedMask = 0x0, .GreenMask = 0x0, .BlueMask = 0x0},
            .expected_format = ZBI_PIXEL_FORMAT_NONE,
        },
        {
            .test_name = "no_mask",
            .format = PixelBlueGreenRedReserved8BitPerColor,
            .expected_format = ZBI_PIXEL_FORMAT_RGB_X888,
        },
    }),
    [](testing::TestParamInfo<PixelFormatTest::ParamType> const &info) {
      return info.param.test_name;
    });

TEST_F(BootZbiItemTest, SystemTableTest) {
  auto cleanup = SetupEfiGlobalState();
  ASSERT_EQ(zbi_init(buffer().data(), buffer().size()), ZBI_RESULT_OK);
  AbrSlotIndex slot = kAbrSlotIndexA;
  ASSERT_TRUE(AddGigabootZbiItems(reinterpret_cast<zbi_header_t *>(buffer().data()),
                                  buffer().size(), &slot, &context()));

  std::vector<zbitl::ByteView> items = FindItems(buffer().data(), ZBI_TYPE_EFI_SYSTEM_TABLE);
  ASSERT_EQ(items.size(), 1ULL);

  ASSERT_EQ(*reinterpret_cast<const efi_system_table *const *>(items[0].data()), gEfiSystemTable);
}

}  // namespace

}  // namespace gigaboot
