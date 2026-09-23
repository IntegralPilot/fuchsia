// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-shim-utils/commandline-bootloader-file-item.h>
#include <lib/boot-shim/boot-shim.h>
#include <lib/fit/defer.h>
#include <lib/zbitl/image.h>

#include <algorithm>
#include <array>
#include <string>
#include <string_view>

#include <zxtest/zxtest.h>

namespace {

template <typename Zbi, typename Pred>
bool HasZbiItem(Zbi&& zbi, Pred&& pred) {
  auto cleanup = fit::defer([&zbi]() { zbi.ignore_error(); });
  return std::any_of(zbi.begin(), zbi.end(),
                     [pred](auto item) -> bool { return pred(*item.header, item.payload); });
}

// Compare bitwise equality of a string_view and ByteView.
bool operator==(std::string_view sv, zbitl::ByteView bv) {
  return sv.size() == bv.size_bytes() && memcmp(sv.data(), bv.data(), sv.size()) == 0;
}

TEST(CommandlineBootloaderFileItemTest, NoChunks) {
  constexpr std::string_view kCmdline = "foo=bar baz=qux";
  constexpr std::string_view kPrefix = "ssh_creds=";
  constexpr std::string_view kFilename = "ssh.authorized_keys";

  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  boot_shim::BootShim<CommandlineBootloaderFileItem> shim("test-shim", stdout);
  shim.Get<CommandlineBootloaderFileItem>().Init(kCmdline, kPrefix, kFilename);

  EXPECT_EQ(shim.Get<CommandlineBootloaderFileItem>().size_bytes(), 0u);
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  ASSERT_FALSE(HasZbiItem(image, [](const zbi_header_t& header, zbitl::ByteView payload) {
    return header.type == ZBI_TYPE_BOOTLOADER_FILE;
  }));
}

TEST(CommandlineBootloaderFileItemTest, SingleChunk) {
  constexpr std::string_view kPrefix = "ssh_creds=";
  // "Zm9vIGJhcg==" -> "foo bar"
  constexpr std::string_view kCmdline = "ssh_creds=Zm9vIGJhcg==";
  constexpr std::string_view kFilename = "ssh.authorized_keys";
  constexpr std::string_view kExpected = "\x13ssh.authorized_keysfoo bar";

  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  boot_shim::BootShim<CommandlineBootloaderFileItem> shim("test-shim", stdout);
  shim.Get<CommandlineBootloaderFileItem>().Init(kCmdline, kPrefix, kFilename);

  EXPECT_GT(shim.Get<CommandlineBootloaderFileItem>().size_bytes(), 0u);
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  ASSERT_TRUE(HasZbiItem(image, [&](const zbi_header_t& header, zbitl::ByteView payload) {
    return (header.type == ZBI_TYPE_BOOTLOADER_FILE && payload == kExpected);
  }));
}

TEST(CommandlineBootloaderFileItemTest, MultipleChunks) {
  constexpr std::string_view kPrefix = "data=";
  // "Zm9vIGJ" + "hciBiYXo=" -> "foo bar baz"
  constexpr std::string_view kCmdline = "data=Zm9vIGJ data=hciBiYXo=";
  constexpr std::string_view kFilename = "test_filename";
  constexpr std::string_view kExpected = "\x0Dtest_filenamefoo bar baz";

  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  boot_shim::BootShim<CommandlineBootloaderFileItem> shim("test-shim", stdout);
  shim.Get<CommandlineBootloaderFileItem>().Init(kCmdline, kPrefix, kFilename);

  EXPECT_GT(shim.Get<CommandlineBootloaderFileItem>().size_bytes(), 0u);
  ASSERT_TRUE(shim.AppendItems(image).is_ok());

  ASSERT_TRUE(HasZbiItem(image, [&](const zbi_header_t& header, zbitl::ByteView payload) {
    return (header.type == ZBI_TYPE_BOOTLOADER_FILE && payload == kExpected);
  }));
}

TEST(CommandlineBootloaderFileItemTest, InvalidBase64) {
  constexpr std::string_view kCmdline = "ssh_creds=!!!!!";
  constexpr std::string_view kPrefix = "ssh_creds=";
  constexpr std::string_view kFilename = "ssh.authorized_keys";

  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  boot_shim::BootShim<CommandlineBootloaderFileItem> shim("test-shim", stdout);
  shim.Get<CommandlineBootloaderFileItem>().Init(kCmdline, kPrefix, kFilename);

  EXPECT_GT(shim.Get<CommandlineBootloaderFileItem>().size_bytes(), 0u);
  ASSERT_FALSE(shim.AppendItems(image).is_ok());
}

TEST(CommandlineBootloaderFileItemTest, FilenameTooLarge) {
  constexpr std::string_view kCmdline = "ssh_creds=Zm9vIGJhcg==";
  constexpr std::string_view kPrefix = "ssh_creds=";
  std::string huge_filename(256, 'A');

  std::array<std::byte, 512> image_buffer;
  zbitl::Image<std::span<std::byte>> image(image_buffer);
  ASSERT_TRUE(image.clear().is_ok());

  boot_shim::BootShim<CommandlineBootloaderFileItem> shim("test-shim", stdout);
  shim.Get<CommandlineBootloaderFileItem>().Init(kCmdline, kPrefix, huge_filename);

  EXPECT_EQ(shim.Get<CommandlineBootloaderFileItem>().size_bytes(), 0u);
  ASSERT_FALSE(shim.AppendItems(image).is_ok());
}

}  // namespace
