// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim-utils/devicetree-bootloader-files-item.h>
#include <lib/boot-shim/devicetree-boot-shim.h>
#include <lib/boot-shim/devicetree.h>
#include <lib/fit/defer.h>
#include <lib/zbitl/image.h>

#include <zxtest/zxtest.h>

extern const uint8_t kChosenBootloaderFilesDtbStart[] __asm__(
    "embedded_blob.start.chosen_bootloader_files_blob");
extern const uint8_t kChosenNoBootloaderFilesDtbStart[] __asm__(
    "embedded_blob.start.chosen_no_bootloader_files_blob");

namespace {

TEST(BootloaderFilesItemTest, ParsesAll) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());
  devicetree::ByteView fdt_blob(kChosenBootloaderFilesDtbStart, -1);
  devicetree::Devicetree fdt(fdt_blob);
  boot_shim::DevicetreeBootShim<DevicetreeBootloaderFilesItem> shim("test", fdt);
  __attribute__((aligned(ZBI_ALIGNMENT))) std::byte scratch_buffer[1024];
  shim.Get<DevicetreeBootloaderFilesItem>().SetScratchBuffer(scratch_buffer);
  ASSERT_TRUE(shim.Init());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  const std::pair<std::span<const char>, std::span<const char>> kExpected[] = {
      {"file-1", "file-1-data"},
      {"file-2", "file-2-data"},
  };

  size_t idx = 0;
  for (auto [header, payload] : image) {
    if (header->type != ZBI_TYPE_BOOTLOADER_FILE) {
      continue;
    }
    size_t name_len = static_cast<size_t>(payload.data()[0]);
    const char* s = reinterpret_cast<const char*>(payload.data() + 1);
    EXPECT_EQ(kExpected[idx].first.size() - 1, name_len);
    auto a = std::span<const char>{s, name_len};
    EXPECT_EQ(std::string(kExpected[idx].first.begin(), kExpected[idx].first.end() - 1),
              std::string(a.begin(), a.end()));
    EXPECT_EQ(std::string(kExpected[idx].second.begin(), kExpected[idx].second.end()),
              std::string(s + name_len, s + payload.size() - 1));
    idx++;
  }

  ASSERT_EQ(idx, 2);
}

TEST(BootloaderFilesItemTest, NoBootloaderFiles) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());
  devicetree::ByteView fdt_blob(kChosenNoBootloaderFilesDtbStart, -1);
  devicetree::Devicetree fdt(fdt_blob);
  boot_shim::DevicetreeBootShim<DevicetreeBootloaderFilesItem> shim("test", fdt);
  __attribute__((aligned(ZBI_ALIGNMENT))) std::byte scratch_buffer[1024];
  shim.Get<DevicetreeBootloaderFilesItem>().SetScratchBuffer(scratch_buffer);
  ASSERT_TRUE(shim.Init());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  for (auto [header, payload] : image) {
    ASSERT_NE(header->type, ZBI_TYPE_BOOTLOADER_FILE);
  }
}

TEST(BootloaderFilesItemTest, UnalignedBuffer) {
  std::array<std::byte, 1024> image_buffer;
  std::vector<void*> allocs;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());
  devicetree::ByteView fdt_blob(kChosenBootloaderFilesDtbStart, -1);
  devicetree::Devicetree fdt(fdt_blob);
  boot_shim::DevicetreeBootShim<DevicetreeBootloaderFilesItem> shim("test", fdt);
  __attribute__((aligned(ZBI_ALIGNMENT))) std::byte scratch_buffer[1024];
  shim.Get<DevicetreeBootloaderFilesItem>().SetScratchBuffer({scratch_buffer + 1, 1023});
  ASSERT_TRUE(shim.Init());
  auto clear_errors = fit::defer([&]() { image.ignore_error(); });
  ASSERT_TRUE(shim.AppendItems(image).is_ok());
  for (auto [header, payload] : image) {
    ASSERT_NE(header->type, ZBI_TYPE_BOOTLOADER_FILE);
  }
}

}  // namespace
