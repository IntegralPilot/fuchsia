// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zbi-format/kernel.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbi/zbi.h>
#include <lib/zircon_boot/zbi_utils.h>

#include <vector>

#include <zxtest/zxtest.h>

namespace {

constexpr uint32_t ZbiAlign(uint32_t n) { return ((n + ZBI_ALIGNMENT - 1) & -ZBI_ALIGNMENT); }

TEST(ZbiTests, ZbiFileItemAppend) {
  constexpr char kFileName[] = "file name";
  constexpr size_t kFileNameLen = sizeof(kFileName) - 1;
  constexpr char kFileContent[] = "file content";
  struct {
    zbi_header_t header;
    zbi_header_t file_hdr;
    uint8_t file_payload[ZbiAlign(1 + kFileNameLen + sizeof(kFileContent))];
  } test_zbi;
  ASSERT_EQ(zbi_init(&test_zbi, sizeof(test_zbi)), ZBI_RESULT_OK);
  ASSERT_EQ(AppendZbiFile(&test_zbi.header, sizeof(test_zbi), kFileName, kFileContent,
                          sizeof(kFileContent)),
            ZBI_RESULT_OK);

  ASSERT_EQ(test_zbi.file_hdr.type, ZBI_TYPE_BOOTLOADER_FILE);
  ASSERT_EQ(test_zbi.file_hdr.extra, 0);
  ASSERT_EQ(test_zbi.file_hdr.length, 1 + kFileNameLen + sizeof(kFileContent));
  const uint8_t* payload = test_zbi.file_payload;
  ASSERT_EQ(payload[0], kFileNameLen);
  ASSERT_BYTES_EQ(payload + 1, kFileName, kFileNameLen);
  ASSERT_BYTES_EQ(payload + 1 + kFileNameLen, kFileContent, sizeof(kFileContent));
}

TEST(ZbiTests, NameTooLong) {
  std::string name(257, 'a');
  ASSERT_EQ(AppendZbiFile(nullptr, 0, name.data(), nullptr, 0), ZBI_RESULT_ERROR);
}

TEST(ZbiTests, AppendZbiFilePayloadLengthOverflow) {
  std::string name(10, 'a');
  size_t overflow_size = std::numeric_limits<size_t>::max() - name.size();
  ASSERT_EQ(AppendZbiFile(nullptr, 0, name.data(), nullptr, overflow_size), ZBI_RESULT_TOO_BIG);
}

constexpr size_t kKernelEntryValue = 1;
constexpr size_t kKernelReserveMemeorySizeValue = 64;

struct TestZbiKernel {
  zbi_header_t hdr_file;
  zbi_header_t hdr_kernel;
  zbi_kernel_t data_kernel;
  uint8_t kernel_payload[64];

  static constexpr size_t RequiredBufferSize() {
    return sizeof(TestZbiKernel) + kKernelReserveMemeorySizeValue;
  }
};

struct TestZirconKernelImage {
  TestZbiKernel kernel;
  zbi_header_t zbi_item;
  uint8_t zbi_item_payload[64];
};

struct RelocateToTailExpectedLayout {
  TestZirconKernelImage image;
  TestZbiKernel relocated __attribute__((__aligned__(ZIRCON_BOOT_KERNEL_ALIGN)));

  static constexpr size_t RequiredZbiCapacity() {
    return offsetof(RelocateToTailExpectedLayout, relocated) + sizeof(TestZbiKernel) +
           kKernelReserveMemeorySizeValue;
  }
};

TestZirconKernelImage CreateTestZirconKernelImage() {
  TestZirconKernelImage test_kernel;
  EXPECT_EQ(zbi_init(&test_kernel, sizeof(test_kernel)), ZBI_RESULT_OK);
  void* payload;
  EXPECT_EQ(
      zbi_create_entry(
          &test_kernel, sizeof(test_kernel),
#ifdef __aarch64__
          ZBI_TYPE_KERNEL_ARM64,
#elif defined(__x86_64__) || defined(__i386__)
          ZBI_TYPE_KERNEL_X64,
#elif defined(__riscv)
          ZBI_TYPE_KERNEL_RISCV64,
#else
#error "what architecture?"
#endif
          0, 0, sizeof(test_kernel.kernel.data_kernel) + sizeof(test_kernel.kernel.kernel_payload),
          &payload),
      ZBI_RESULT_OK);
  test_kernel.kernel.data_kernel.entry = kKernelEntryValue;
  test_kernel.kernel.data_kernel.reserve_memory_size = kKernelReserveMemeorySizeValue;
  memset(test_kernel.kernel.kernel_payload, 0xaa, sizeof(test_kernel.kernel.kernel_payload));

  // Create a zbi items payload
  EXPECT_EQ(zbi_create_entry(&test_kernel, sizeof(test_kernel), ZBI_TYPE_CMDLINE, 0, 0,
                             sizeof(test_kernel.zbi_item_payload), &payload),
            ZBI_RESULT_OK);
  EXPECT_EQ(test_kernel.kernel.hdr_file.length,
            sizeof(TestZirconKernelImage) - sizeof(zbi_header_t));
  memset(test_kernel.zbi_item_payload, 0x55, sizeof(test_kernel.zbi_item_payload));
  return test_kernel;
}

TEST(ZbiTests, RelocateKernel) {
  TestZirconKernelImage test_kernel = CreateTestZirconKernelImage();
  uint8_t relocate_buffer[TestZbiKernel::RequiredBufferSize()]
      __attribute__((__aligned__(ZIRCON_BOOT_KERNEL_ALIGN)));
  size_t buffer_size = sizeof(relocate_buffer);
  ASSERT_EQ(
      static_cast<uint8_t*>(RelocateKernel(reinterpret_cast<const zbi_header_t*>(&test_kernel),
                                           relocate_buffer, &buffer_size)),
      relocate_buffer + test_kernel.kernel.data_kernel.entry);
  TestZbiKernel relocated_kernel;
  memcpy(&relocated_kernel, relocate_buffer, sizeof(relocated_kernel));
  // Relocated container header is updated to required boot size (factoring in reserved memory).
  ASSERT_EQ(relocated_kernel.hdr_file.length, sizeof(TestZbiKernel) - sizeof(zbi_header_t));
  // All other payload should remain the same except the container header.
  ASSERT_EQ(memcmp(&test_kernel.kernel.hdr_kernel, &relocated_kernel.hdr_kernel,
                   sizeof(relocated_kernel) - sizeof(zbi_header_t)),
            0);
}

TEST(ZbiTests, RelocateKernelToTail) {
  TestZirconKernelImage test_kernel = CreateTestZirconKernelImage();
  static RelocateToTailExpectedLayout actual;
  static uint8_t buffer[RelocateToTailExpectedLayout::RequiredZbiCapacity()]
      __attribute__((__aligned__(ZIRCON_BOOT_KERNEL_ALIGN)));
  memset(buffer, 0, sizeof(buffer));
  memcpy(buffer, &test_kernel, sizeof(test_kernel));

  size_t buffer_size = sizeof(buffer);
  void* entry = RelocateKernelToTail(reinterpret_cast<zbi_header_t*>(buffer), &buffer_size);
  ASSERT_EQ(entry, buffer + offsetof(RelocateToTailExpectedLayout, relocated) + kKernelEntryValue);
  memcpy(&actual, buffer, sizeof(buffer));
  // Original container remains the same.
  ASSERT_EQ(memcmp(&actual.image, &test_kernel, sizeof(test_kernel)), 0);
  // Relocated container header is updated to required boot size (factoring in reserved memory).
  ASSERT_EQ(actual.relocated.hdr_file.length, sizeof(TestZbiKernel) - sizeof(zbi_header_t));
  // All other kernel payload should remain the same except the container header.
  ASSERT_EQ(memcmp(&test_kernel.kernel.hdr_kernel, &actual.relocated.hdr_kernel,
                   sizeof(TestZbiKernel) - sizeof(zbi_header_t)),
            0);
}

TEST(ZbiTests, RelocateKernelInvalidZbiType) {
  TestZirconKernelImage test_kernel = CreateTestZirconKernelImage();
  test_kernel.kernel.hdr_file.type = ZBI_TYPE_DISCARD;
  uint8_t relocate_buffer[sizeof(TestZirconKernelImage)];
  size_t buffer_size = sizeof(relocate_buffer);
  ASSERT_EQ(RelocateKernel(reinterpret_cast<const zbi_header_t*>(&test_kernel), relocate_buffer,
                           &buffer_size),
            nullptr);
}

TEST(ZbiTests, RelocateKernelUnalignedBuffer) {
  TestZirconKernelImage test_kernel = CreateTestZirconKernelImage();
  test_kernel.kernel.hdr_file.type = ZBI_TYPE_DISCARD;
  uint8_t relocate_buffer[sizeof(TestZirconKernelImage) + 1]
      __attribute__((__aligned__(ZIRCON_BOOT_KERNEL_ALIGN)));
  size_t buffer_size = sizeof(relocate_buffer);
  ASSERT_EQ(RelocateKernel(reinterpret_cast<const zbi_header_t*>(&test_kernel), relocate_buffer + 1,
                           &buffer_size),
            nullptr);
}

TEST(ZbiTests, RelocateKernelInvalidZbiKernelType) {
  TestZirconKernelImage test_kernel = CreateTestZirconKernelImage();
  test_kernel.kernel.hdr_kernel.type = ZBI_TYPE_DISCARD;
  uint8_t relocate_buffer[sizeof(TestZirconKernelImage)]
      __attribute__((__aligned__(ZIRCON_BOOT_KERNEL_ALIGN)));
  size_t buffer_size = sizeof(relocate_buffer);
  ASSERT_EQ(RelocateKernel(reinterpret_cast<const zbi_header_t*>(&test_kernel), relocate_buffer,
                           &buffer_size),
            nullptr);
}

TEST(ZbiTests, RelocateKernelKernelTooLarge) {
  TestZirconKernelImage test_kernel = CreateTestZirconKernelImage();
  uint8_t relocate_buffer[sizeof(TestZbiKernel) + kKernelReserveMemeorySizeValue - 1]
      __attribute__((__aligned__(ZIRCON_BOOT_KERNEL_ALIGN)));
  ;
  size_t buffer_size = sizeof(relocate_buffer);
  ASSERT_EQ(RelocateKernel(reinterpret_cast<const zbi_header_t*>(&test_kernel), relocate_buffer,
                           &buffer_size),
            nullptr);
  ASSERT_EQ(buffer_size, sizeof(relocate_buffer) + 1);
}

TEST(ZbiTests, RelocateKernelToTailBufferToSmall) {
  TestZirconKernelImage test_kernel = CreateTestZirconKernelImage();
  static uint8_t buffer[RelocateToTailExpectedLayout::RequiredZbiCapacity() - 1]
      __attribute__((__aligned__(ZIRCON_BOOT_KERNEL_ALIGN)));
  memset(buffer, 0, sizeof(buffer));
  memcpy(buffer, &test_kernel, sizeof(test_kernel));
  size_t buffer_size = sizeof(buffer);
  ASSERT_EQ(RelocateKernelToTail(reinterpret_cast<zbi_header_t*>(buffer), &buffer_size), nullptr);
  ASSERT_EQ(buffer_size, sizeof(buffer) + 1);
}

}  // namespace
