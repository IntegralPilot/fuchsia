// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.acpi.tables/cpp/wire.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/channel.h>
#include <lib/zx/vmo.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include <zxtest/zxtest.h>

// These are integration tests of the x86 board drive which check that exported services
// are correctly functioning.

namespace {

constexpr uint32_t kGiB = 1024 * 1024 * 1024;

using fuchsia_acpi_tables::Tables;
using fuchsia_acpi_tables::wire::TableInfo;

const char kAcpiDevicePath[] = "/dev/sys/platform/pt/acpi";

// Open up channel to ACPI device.
zx::result<fidl::ClientEnd<Tables>> OpenChannel() {
  zx::result channel = device_watcher::RecursiveWaitForFile(kAcpiDevicePath);
  if (channel.is_error()) {
    return channel.take_error();
  }

  return zx::ok(fidl::ClientEnd<Tables>(std::move(channel.value())));
}

// Convert a fidl::Array<uint8_t, n> type to a std::string.
template <uint64_t N>
std::string SignatureToString(const fidl::Array<uint8_t, N>& array) {
  return std::string(reinterpret_cast<const char*>(array.data()), array.size());
}

// Convert a string type to a fidl::Array<uint8_t, 4>.
fidl::Array<uint8_t, 4> StringToSignature(std::string_view str) {
  fidl::Array<uint8_t, 4> result;
  ZX_ASSERT(str.size() >= 4);
  memcpy(result.data(), str.data(), 4);
  return result;
}

// Create a pair of VMOs for transferring data to and from the x86 board driver.
//
// |size| specifies how much memory to use for the VMO. By default, we allocate
// 1 GiB to ensure that we have more than enough space: the kernel won't actually
// allocate this memory until needed, though, so in practice we will only use
// a tiny fraction of this (A typical size for the DSDT table is ~100kiB.)
std::tuple<zx::vmo, zx::vmo> CreateVmoPair(size_t size = 1 * kGiB) {
  zx::vmo a, b;
  ZX_ASSERT(zx::vmo::create(size, /*options=*/0, &a) == ZX_OK);
  ZX_ASSERT(a.duplicate(ZX_RIGHT_SAME_RIGHTS, &b) == ZX_OK);
  return std::make_tuple(std::move(a), std::move(b));
}

TEST(X86Board, Connect) {
  zx::result dev = OpenChannel();
  ASSERT_OK(dev.status_value());

  EXPECT_TRUE(dev.value().channel().is_valid());
}

TEST(X86Board, ListTableEntries) {
  zx::result dev = OpenChannel();
  ASSERT_OK(dev.status_value());

  fidl::WireResult<Tables::ListTableEntries> result =
      fidl::WireCall(std::move(dev.value()))->ListTableEntries();
  ASSERT_TRUE(result.ok());
  ASSERT_TRUE(result->is_ok());
  const auto& response = *result->value();

  // We expect to find at least a DSDT entry.
  EXPECT_GE(response.entries.size(), 1);
  bool found_dsdt = false;
  for (const TableInfo& info : response.entries) {
    if (SignatureToString(info.name) != "DSDT") {
      continue;
    }
    EXPECT_GE(info.size, 1);
    found_dsdt = true;
  }
  EXPECT_TRUE(found_dsdt);
}

TEST(X86Board, ReadNamedTable) {
  zx::result dev = OpenChannel();
  ASSERT_OK(dev.status_value());

  // Read the system's DSDT entry. Every system should have one of these.
  auto [vmo, vmo_copy] = CreateVmoPair();
  fidl::WireResult<Tables::ReadNamedTable> result =
      fidl::WireCall(std::move(dev.value()))
          ->ReadNamedTable(StringToSignature("DSDT"), 0, std::move(vmo_copy));
  ASSERT_TRUE(result.ok());
  ASSERT_TRUE(result->is_ok());
  const auto& response = *result->value();

  // Ensure the size looks sensible.
  ASSERT_GE(response.size, 4);

  // Ensure the first four bytes match "DSDT".
  char buff[4];
  ASSERT_OK(vmo.read(buff, /*offset=*/0, /*length=*/4));
  EXPECT_EQ("DSDT", std::string_view(buff, 4));
}

TEST(X86Board, InvalidTableName) {
  zx::result dev = OpenChannel();
  ASSERT_OK(dev.status_value());

  // Read an invalid entry.
  auto [vmo, vmo_copy] = CreateVmoPair();
  fidl::WireResult<Tables::ReadNamedTable> result =
      fidl::WireCall(std::move(dev.value()))
          ->ReadNamedTable(StringToSignature("???\n"), 0, std::move(vmo_copy));
  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_error());
  EXPECT_EQ(result->error_value(), ZX_ERR_NOT_FOUND);
}

TEST(X86Board, InvalidIndexNumber) {
  zx::result dev = OpenChannel();
  ASSERT_OK(dev.status_value());

  // Read a large index of the DSDT table. We should have a DSDT table, but really only
  // have 1 of them.
  auto [vmo, vmo_copy] = CreateVmoPair();
  fidl::WireResult<Tables::ReadNamedTable> result =
      fidl::WireCall(std::move(dev.value()))
          ->ReadNamedTable(StringToSignature("DSDT"), 1234, std::move(vmo_copy));
  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_error());
  EXPECT_EQ(result->error_value(), ZX_ERR_NOT_FOUND);
}

TEST(X86Board, VmoTooSmall) {
  zx::result dev = OpenChannel();
  ASSERT_OK(dev.status_value());

  // Only allocate a VMO with 3 bytes backing it.
  auto [vmo, vmo_copy] = CreateVmoPair(/*size=*/3);
  fidl::WireResult<Tables::ReadNamedTable> result =
      fidl::WireCall(std::move(dev.value()))
          ->ReadNamedTable(StringToSignature("DSDT"), 0, std::move(vmo_copy));
  ASSERT_TRUE(result.ok());
  EXPECT_TRUE(result->is_error());
  EXPECT_EQ(result->error_value(), ZX_ERR_OUT_OF_RANGE);
}

TEST(X86Board, ReadOnlyVmoSent) {
  zx::result dev = OpenChannel();
  ASSERT_OK(dev.status_value());

  // Send a read-only VMO.
  auto [vmo, vmo_copy] = CreateVmoPair();
  zx::vmo read_only_vmo;
  ZX_ASSERT(vmo_copy.replace(ZX_RIGHT_NONE, &read_only_vmo) == ZX_OK);
  fidl::WireResult<Tables::ReadNamedTable> result =
      fidl::WireCall(std::move(dev.value()))
          ->ReadNamedTable(StringToSignature("DSDT"), 0, std::move(read_only_vmo));
  EXPECT_EQ(result.status(), ZX_ERR_ACCESS_DENIED);
}

TEST(X86Board, InvalidObject) {
  zx::result dev = OpenChannel();
  ASSERT_OK(dev.status_value());

  // Send something that is not a VMO.
  zx::channel a, b;
  zx::channel::create(/*flags=*/0, &a, &b);
  fidl::WireResult<Tables::ReadNamedTable> result =
      fidl::WireCall(std::move(dev.value()))
          ->ReadNamedTable(StringToSignature("DSDT"), 0, zx::vmo(a.release()));
  // FIDL detects that a channel is being sent as a VMO handle.
  ASSERT_FALSE(result.ok());
}

}  // namespace
