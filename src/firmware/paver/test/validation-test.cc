// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/validation.h"

#include <lib/zbi-format/zbi.h>
#include <zircon/errors.h>

#include <span>

#include <phys/zbi.h>
#include <zxtest/zxtest.h>

#include "src/firmware/paver/device-partitioner.h"
#include "src/firmware/paver/test/test-utils.h"

namespace paver {

namespace {

TEST(IsValidKernelZbi, EmptyData) {
  ASSERT_FALSE(IsValidKernelZbi(Arch::kX64, std::span<uint8_t>()));
}

TEST(IsValidKernelZbi, MinimalValid) {
  std::span<uint8_t> data;
  ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 0, &header, &data);
  ASSERT_TRUE(IsValidKernelZbi(Arch::kX64, data));
}

TEST(IsValidKernelZbi, DataTooSmall) {
  std::span<uint8_t> data;
  ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 1024, &header, &data);
  header->hdr_file.length += 1;
  ASSERT_FALSE(IsValidKernelZbi(Arch::kX64, data));
}

TEST(IsValidKernelZbi, DataTooBig) {
  std::span<uint8_t> data;
  ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 1024, &header, &data);
  header->hdr_file.length = 0xffff'ffffu;
  ASSERT_FALSE(IsValidKernelZbi(Arch::kX64, data));
}

TEST(IsValidKernelZbi, KernelDataTooSmall) {
  std::span<uint8_t> data;
  ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 1024, &header, &data);
  header->hdr_kernel.length += 1;
  ASSERT_FALSE(IsValidKernelZbi(Arch::kX64, data));
}

TEST(IsValidKernelZbi, ValidWithPayload) {
  std::span<uint8_t> data;
  ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 1024, &header, &data);
  ASSERT_TRUE(IsValidKernelZbi(Arch::kX64, data));
}

TEST(IsValidKernelZbi, InvalidArch) {
  std::span<uint8_t> data;
  ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 0, &header, &data);
  ASSERT_FALSE(IsValidKernelZbi(Arch::kArm64, data));
}

TEST(IsValidKernelZbi, InvalidMagic) {
  std::span<uint8_t> data;
  ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 0, &header, &data);
  header->hdr_file.magic = 0;
  ASSERT_FALSE(IsValidKernelZbi(Arch::kX64, data));
}

TEST(IsValidKernelZbi, ValidCrc) {
  std::span<uint8_t> data;
  ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 0, &header, &data);
  header->hdr_kernel.flags |= ZBI_FLAGS_CRC32;
  header->data_kernel.entry = 0x1122334455667788;
  header->data_kernel.reserve_memory_size = 0xaabbccdd12345678;
  header->hdr_kernel.crc32 = 0x8b8e6cfc;  // == crc32({header->data_kernel})
  ASSERT_TRUE(IsValidKernelZbi(Arch::kX64, data));
}

TEST(IsValidKernelZbi, InvalidCrc) {
  std::span<uint8_t> data;
  ZbiKernelImage* header;
  auto array = CreateZbiHeader(Arch::kX64, 0, &header, &data);
  header->hdr_kernel.flags |= ZBI_FLAGS_CRC32;
  header->data_kernel.entry = 0x1122334455667788;
  header->data_kernel.reserve_memory_size = 0xaabbccdd12345678;
  header->hdr_kernel.crc32 = 0xffffffff;
  ASSERT_FALSE(IsValidKernelZbi(Arch::kX64, data));
}

static std::span<const uint8_t> StringToSpan(const std::string& data) {
  return std::span<const uint8_t>(reinterpret_cast<const uint8_t*>(data.data()), data.size());
}

TEST(IsValidChromeOsKernel, TooSmall) {
  EXPECT_FALSE(IsValidChromeOsKernel(StringToSpan("")));
  EXPECT_FALSE(IsValidChromeOsKernel(StringToSpan("C")));
  EXPECT_FALSE(IsValidChromeOsKernel(StringToSpan("CHROMEO")));
}

TEST(IsValidChromeOsKernel, IncorrectMagic) {
  EXPECT_FALSE(IsValidChromeOsKernel(StringToSpan("CHROMEOX")));
}

TEST(IsValidChromeOsKernel, MinimalValid) {
  EXPECT_TRUE(IsValidChromeOsKernel(StringToSpan("CHROMEOS")));
}

TEST(IsValidChromeOsKernel, ExcessData) {
  EXPECT_TRUE(IsValidChromeOsKernel(StringToSpan("CHROMEOS-1234")));
}

TEST(IsValidAndroidKernel, Validity) {
  EXPECT_TRUE(IsValidAndroidKernel(StringToSpan("ANDROID!")));
  EXPECT_FALSE(IsValidAndroidKernel(StringToSpan("VNDRBOOT")));
}

TEST(IsValidAndroidVendorKernel, Validity) {
  EXPECT_TRUE(IsValidAndroidVendorKernel(StringToSpan("VNDRBOOT")));
  EXPECT_FALSE(IsValidAndroidVendorKernel(StringToSpan("ANDROID!")));
}

}  // namespace
}  // namespace paver
