// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/stdcompat/array.h>

#include <string_view>
#include <vector>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

namespace {

bool CheckContents(std::string_view contents, fbl::unique_fd file) {
  std::vector<char> actual;
  actual.resize(contents.size());
  size_t acc_offset = 0;
  while (auto read_bytes =
             read(file.get(), actual.data() + acc_offset, actual.size() - acc_offset)) {
    acc_offset += read_bytes;
  }
  return acc_offset == contents.size() &&
         memcmp(actual.data(), contents.data(), actual.size()) == 0;
}

}  // namespace

TEST(EarlyBootInstrumentationTest, HasLogsUnderLogs) {
  auto entries = cpp20::to_array<std::string_view>(
      {"symbolizer.log", "physboot.log", "physload.log", "foo-bar.log"});
  for (auto entry : entries) {
    std::string path = "/logs/" + std::string(entry);
    fbl::unique_fd file(open(path.c_str(), O_RDONLY));
    ASSERT_TRUE(file);
    ASSERT_TRUE(CheckContents(entry, std::move(file)));
  }
}

TEST(EarlyBootInstrumentationTest, HasKernelInDynamic) {
  fbl::unique_fd kernel_file(open("/debugdata/llvm-profile/dynamic/zircon.profraw", O_RDONLY));
  ASSERT_TRUE(kernel_file);
  ASSERT_TRUE(CheckContents("kernel", std::move(kernel_file)));
}

TEST(EarlyBootInstrumentationTest, HasPhysbootInStatic) {
  fbl::unique_fd physboot_file(open("/debugdata/llvm-profile/static/physboot.profraw", O_RDONLY));
  ASSERT_TRUE(physboot_file);
  ASSERT_TRUE(CheckContents("physboot", std::move(physboot_file)));
}

TEST(EarlyBootInstrumentationTest, HasSvcStashDataWithLlvmProfile) {
  fbl::unique_fd static_file(open("/debugdata/llvm-profile/static/0-0.profraw", O_RDONLY));
  ASSERT_TRUE(static_file);
  ASSERT_TRUE(CheckContents("1234", std::move(static_file)));

  fbl::unique_fd dynamic_file(open("/debugdata/llvm-profile/dynamic/0-1.profraw", O_RDONLY));
  ASSERT_TRUE(dynamic_file);
  ASSERT_TRUE(CheckContents("567890123", std::move(dynamic_file)));
}

TEST(EarlyBootInstrumentationTest, HasSvcStashDataWithCustomSink) {
  fbl::unique_fd static_file(open("/debugdata/my-custom-sink/static/0-2.custom", O_RDONLY));
  ASSERT_TRUE(static_file);
  ASSERT_TRUE(CheckContents("789", std::move(static_file)));

  fbl::unique_fd dynamic_file(open("/debugdata/my-custom-sink/dynamic/0-3", O_RDONLY));
  ASSERT_TRUE(dynamic_file);
  ASSERT_TRUE(CheckContents("43218765", std::move(dynamic_file)));
}

TEST(EarlyBootInstrumentationTest, HasLogsFromZbiDebugDataItem) {
  auto entries = cpp20::to_array<std::string_view>({"bar-fooz.log", "llvm-profile-foo.log"});
  for (auto entry : entries) {
    std::string path = "/logs/" + std::string(entry);
    fbl::unique_fd file(open(path.c_str(), O_RDONLY));
    ASSERT_TRUE(file);
    ASSERT_TRUE(CheckContents("Fooz Barz", std::move(file)));
  }
}

TEST(EarlyBootInstrumentationTest, HasLlvmProfileSinkFromZbiDebugDataItem) {
  fbl::unique_fd static_file(open("/debugdata/llvm-profile/static/foo", O_RDONLY));
  ASSERT_TRUE(static_file);
  ASSERT_TRUE(CheckContents("foozbarz", std::move(static_file)));
}

TEST(EarlyBootInstrumentationTest, HasCustomSinkFromZbiDebugDataItem) {
  {
    fbl::unique_fd static_file(open("/debugdata/bar/static/foo", O_RDONLY));
    ASSERT_TRUE(static_file);
    ASSERT_TRUE(CheckContents("foozbarz", std::move(static_file)));
  }

  {
    fbl::unique_fd static_file(open("/debugdata/bar/static/fooz", O_RDONLY));
    ASSERT_TRUE(static_file);
    ASSERT_TRUE(CheckContents("foozbarz", std::move(static_file)));
  }
}
