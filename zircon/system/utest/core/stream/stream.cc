// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <lib/maybe-standalone-test/maybe-standalone.h>
#include <lib/zx/bti.h>
#include <lib/zx/event.h>
#include <lib/zx/iommu.h>
#include <lib/zx/stream.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <limits.h>
#include <stdio.h>
#include <unistd.h>
#include <zircon/syscalls/iommu.h>
#include <zircon/syscalls/object.h>
#include <zircon/system/public/zircon/syscalls.h>
#include <zircon/system/utest/core/pager/userpager.h>
#include <zircon/types.h>

#include <memory>
#include <numeric>
#include <string>
#include <thread>
#include <vector>

#include <zxtest/zxtest.h>

#include "zircon/system/utest/core/vmo/helpers.h"

namespace {

// This value corresponds to the kernel's VmObjectPaged::ReadWriteInternal write batch limit
// (kMaxWriteWaitPages).
static constexpr uint64_t kMaxPagesBatch = 256;

void CheckRights(const zx::stream& stream, zx_rights_t expected_rights, const char* message) {
  zx_info_handle_basic_t info = {};
  EXPECT_OK(stream.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  printf("CheckRights: %s\n", message);
  EXPECT_EQ(expected_rights, info.rights);
}

TEST(StreamTestCase, Create) {
  zx_handle_t raw_stream = ZX_HANDLE_INVALID;
  ASSERT_EQ(ZX_ERR_BAD_HANDLE, zx_stream_create(0, ZX_HANDLE_INVALID, 0, &raw_stream));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  ASSERT_EQ(ZX_ERR_WRONG_TYPE, zx_stream_create(0, event.get(), 0, &raw_stream));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 4, 0, &vmo));
  size_t content_size = 0u;
  ASSERT_OK(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  static_assert(!(ZX_DEFAULT_STREAM_RIGHTS & ZX_RIGHT_WRITE),
                "Streams are not writable by default");
  static_assert(!(ZX_DEFAULT_STREAM_RIGHTS & ZX_RIGHT_READ), "Streams are not readable by default");

  zx::stream stream;

  ASSERT_EQ(ZX_ERR_INVALID_ARGS, zx::stream::create(-42, vmo, 0, &stream));

  ASSERT_OK(zx::stream::create(0, vmo, 0, &stream));
  CheckRights(stream, ZX_DEFAULT_STREAM_RIGHTS, "Default");

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ, vmo, 0, &stream));
  CheckRights(stream, ZX_DEFAULT_STREAM_RIGHTS | ZX_RIGHT_READ, "ZX_STREAM_MODE_READ");

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo, 0, &stream));
  CheckRights(stream, ZX_DEFAULT_STREAM_RIGHTS | ZX_RIGHT_WRITE, "ZX_STREAM_MODE_WRITE");

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, vmo, 0, &stream));
  CheckRights(stream, ZX_DEFAULT_STREAM_RIGHTS | ZX_RIGHT_READ | ZX_RIGHT_WRITE,
              "ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE");

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_APPEND, vmo, 0, &stream));
  CheckRights(stream, ZX_DEFAULT_STREAM_RIGHTS, "ZX_STREAM_MODE_APPEND");

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_APPEND, vmo, 0, &stream));
  CheckRights(stream, ZX_DEFAULT_STREAM_RIGHTS | ZX_RIGHT_READ,
              "ZX_STREAM_MODE_READ | ZX_STREAM_MODE_APPEND");

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND, vmo, 0, &stream));
  CheckRights(stream, ZX_DEFAULT_STREAM_RIGHTS | ZX_RIGHT_WRITE,
              "ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND");

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND,
                               vmo, 0, &stream));
  CheckRights(stream, ZX_DEFAULT_STREAM_RIGHTS | ZX_RIGHT_READ | ZX_RIGHT_WRITE,
              "ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND");

  {
    zx::vmo read_only;
    vmo.duplicate(ZX_RIGHT_READ, &read_only);

    ASSERT_OK(zx::stream::create(0, read_only, 0, &stream));
    CheckRights(stream, ZX_DEFAULT_STREAM_RIGHTS, "read_only: Default");

    ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ, read_only, 0, &stream));
    CheckRights(stream, ZX_DEFAULT_STREAM_RIGHTS | ZX_RIGHT_READ, "read_only: ZX_STREAM_MODE_READ");

    ASSERT_EQ(ZX_ERR_ACCESS_DENIED,
              zx::stream::create(ZX_STREAM_MODE_WRITE, read_only, 0, &stream));
    ASSERT_EQ(ZX_ERR_ACCESS_DENIED, zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE,
                                                       read_only, 0, &stream));
  }

  {
    zx::vmo write_only;
    vmo.duplicate(ZX_RIGHT_WRITE, &write_only);

    ASSERT_OK(zx::stream::create(0, write_only, 0, &stream));
    CheckRights(stream, ZX_DEFAULT_STREAM_RIGHTS, "write_only: Default");

    ASSERT_EQ(ZX_ERR_ACCESS_DENIED,
              zx::stream::create(ZX_STREAM_MODE_READ, write_only, 0, &stream));

    ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, write_only, 0, &stream));
    CheckRights(stream, ZX_DEFAULT_STREAM_RIGHTS | ZX_RIGHT_WRITE,
                "write_only: ZX_STREAM_MODE_WRITE");

    ASSERT_EQ(ZX_ERR_ACCESS_DENIED, zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE,
                                                       write_only, 0, &stream));
  }

  {
    zx::vmo none;
    vmo.duplicate(0, &none);

    ASSERT_OK(zx::stream::create(0, none, 0, &stream));
    CheckRights(stream, ZX_DEFAULT_STREAM_RIGHTS, "none: Default");

    ASSERT_EQ(ZX_ERR_ACCESS_DENIED, zx::stream::create(ZX_STREAM_MODE_READ, none, 0, &stream));
    ASSERT_EQ(ZX_ERR_ACCESS_DENIED, zx::stream::create(ZX_STREAM_MODE_WRITE, none, 0, &stream));
    ASSERT_EQ(ZX_ERR_ACCESS_DENIED,
              zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, none, 0, &stream));
  }
}

TEST(StreamTestCase, Seek) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * 4, 0, &vmo));
  size_t content_size = 42u;
  ASSERT_OK(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  zx::stream stream;
  zx_off_t result = 81u;

  ASSERT_OK(zx::stream::create(0, vmo, 0, &stream));
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, stream.seek(ZX_STREAM_SEEK_ORIGIN_START, 0, &result));

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ, vmo, 9, &stream));
  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_CURRENT, 0, &result));
  EXPECT_EQ(9u, result);

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo, 518, &stream));
  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_CURRENT, 0, &result));
  EXPECT_EQ(518u, result);

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, vmo, 0, &stream));

  ASSERT_EQ(ZX_ERR_INVALID_ARGS, stream.seek(34893, 12, &result));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, stream.seek(ZX_STREAM_SEEK_ORIGIN_START, -10, &result));
  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_START, 10, &result));
  EXPECT_EQ(10u, result);
  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_START, 12, &result));
  EXPECT_EQ(12u, result);

  ASSERT_EQ(ZX_ERR_INVALID_ARGS, stream.seek(ZX_STREAM_SEEK_ORIGIN_CURRENT, -21, &result));
  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_CURRENT, 3, &result));
  EXPECT_EQ(15u, result);
  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_CURRENT, -15, &result));
  EXPECT_EQ(0u, result);
  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_CURRENT, INT64_MAX, &result));
  EXPECT_EQ(static_cast<zx_off_t>(INT64_MAX), result);

  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_CURRENT, 1038, &result));
  EXPECT_EQ(static_cast<zx_off_t>(INT64_MAX) + 1038, result);
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, stream.seek(ZX_STREAM_SEEK_ORIGIN_CURRENT, INT64_MAX, &result));

  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_END, 0, &result));
  EXPECT_EQ(content_size, result);
  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_END, -11, &result));
  EXPECT_EQ(31u, result);
  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_END, -13, &result));
  EXPECT_EQ(29u, result);
  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_END, -content_size, &result));
  EXPECT_EQ(0u, result);
  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_END, 24, &result));
  EXPECT_EQ(66u, result);
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, stream.seek(ZX_STREAM_SEEK_ORIGIN_END, -1238, &result));

  content_size = UINT64_MAX;
  EXPECT_STATUS(ZX_ERR_OUT_OF_RANGE,
                vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  ASSERT_EQ(ZX_ERR_INVALID_ARGS,
            stream.seek(ZX_STREAM_SEEK_ORIGIN_END, std::numeric_limits<int64_t>::min(), &result));

  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_START, 0, nullptr));
}

const char kAlphabet[] = "abcdefghijklmnopqrstuvwxyz";

TEST(StreamTestCase, ReadV) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));
  size_t content_size = 26u;
  ASSERT_OK(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  zx::stream stream;
  char buffer[16] = {};
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = sizeof(buffer),
  };
  size_t actual = 42u;

  ASSERT_OK(zx::stream::create(0, vmo, 0, &stream));
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, stream.readv(0, &vec, 1, &actual));

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ, vmo, 0, &stream));
  vec.capacity = 7u;
  ASSERT_OK(stream.readv(0, &vec, 1, &actual));
  EXPECT_EQ(7u, actual);
  EXPECT_STREQ("abcdefg", buffer);
  memset(buffer, 0, sizeof(buffer));

  ASSERT_EQ(ZX_ERR_INVALID_ARGS, stream.readv(24098, &vec, 1, &actual));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, stream.readv(0, nullptr, 1, &actual));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, stream.readv(0, nullptr, 0, &actual));

  vec.capacity = 3u;
  ASSERT_OK(stream.readv(0, &vec, 1, nullptr));
  EXPECT_STREQ("hij", buffer);
  memset(buffer, 0, sizeof(buffer));

  vec.buffer = nullptr;
  vec.capacity = 7u;
  ASSERT_EQ(ZX_ERR_NOT_FOUND, stream.readv(0, &vec, 1, &actual));
  vec.buffer = buffer;

  const size_t kVectorCount = 7;
  zx_iovec_t multivec[kVectorCount] = {};
  for (size_t i = 0; i < kVectorCount; ++i) {
    multivec[i].buffer = buffer;
    multivec[i].capacity = INT64_MAX;
  }

  ASSERT_EQ(ZX_ERR_INVALID_ARGS, stream.readv(0, multivec, kVectorCount, &actual));

  vec.capacity = sizeof(buffer);
  ASSERT_OK(stream.readv(0, &vec, 1, &actual));
  memset(buffer, 0, sizeof(buffer));

  content_size = 6u;
  ASSERT_OK(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  vec.capacity = 3u;
  actual = 42u;
  ASSERT_OK(stream.readv(0, &vec, 1, &actual));
  EXPECT_EQ(0u, actual);
  memset(buffer, 0, sizeof(buffer));

  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_START, 0, nullptr));
  vec.capacity = 12u;
  actual = 42u;
  ASSERT_OK(stream.readv(0, &vec, 1, &actual));
  EXPECT_EQ(6u, actual);
  EXPECT_STREQ("abcdef", buffer);
  memset(buffer, 0, sizeof(buffer));

  content_size = 26u;
  ASSERT_OK(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  for (size_t i = 0; i < kVectorCount; ++i) {
    multivec[i].buffer = &buffer[i];
    multivec[i].capacity = 1;
  }

  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_START, 0, nullptr));
  ASSERT_OK(stream.readv(0, multivec, kVectorCount, &actual));
  EXPECT_EQ(kVectorCount, actual);
  EXPECT_STREQ("abcdef", buffer);
  memset(buffer, 0, sizeof(buffer));
}

std::string GetData(const zx::vmo& vmo) {
  std::vector<char> buffer(zx_system_get_page_size(), '\0');
  EXPECT_OK(vmo.read(buffer.data(), 0, buffer.size()));
  return std::string(buffer.data());
}

TEST(StreamTestCase, WriteV) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));
  size_t content_size = 26u;
  ASSERT_OK(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  zx::stream stream;
  char buffer[17] = "0123456789ABCDEF";
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = sizeof(buffer),
  };
  size_t actual = 42u;

  ASSERT_OK(zx::stream::create(0, vmo, 0, &stream));
  ASSERT_EQ(ZX_ERR_ACCESS_DENIED, stream.writev(0, &vec, 1, &actual));

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo, 0, &stream));
  vec.capacity = 7u;
  ASSERT_OK(stream.writev(0, &vec, 1, &actual));
  EXPECT_EQ(7u, actual);
  EXPECT_STREQ("0123456hijklmnopqrstuvwxyz", GetData(vmo).c_str());
  ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));

  ASSERT_EQ(ZX_ERR_INVALID_ARGS, stream.writev(24098, &vec, 1, &actual));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, stream.writev(0, nullptr, 1, &actual));
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, stream.writev(0, nullptr, 0, &actual));

  vec.capacity = 3u;
  ASSERT_OK(stream.writev(0, &vec, 1, nullptr));
  EXPECT_STREQ("abcdefg012klmnopqrstuvwxyz", GetData(vmo).c_str());
  ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));

  vec.buffer = nullptr;
  vec.capacity = 7u;
  ASSERT_EQ(ZX_ERR_NOT_FOUND, stream.writev(0, &vec, 1, &actual));
  vec.buffer = buffer;

  const size_t kVectorCount = 7;
  zx_iovec_t multivec[kVectorCount] = {};
  for (size_t i = 0; i < kVectorCount; ++i) {
    multivec[i].buffer = buffer;
    multivec[i].capacity = INT64_MAX;
  }

  ASSERT_EQ(ZX_ERR_INVALID_ARGS, stream.writev(0, multivec, kVectorCount, &actual));

  for (size_t i = 0; i < kVectorCount; ++i) {
    multivec[kVectorCount - i - 1].buffer = &buffer[i];
    multivec[kVectorCount - i - 1].capacity = 1;
  }

  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_START, 0, nullptr));
  ASSERT_OK(stream.writev(0, multivec, kVectorCount, &actual));
  EXPECT_EQ(kVectorCount, actual);
  EXPECT_STREQ("6543210hijklmnopqrstuvwxyz", GetData(vmo).c_str());
  ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));
}

size_t GetContentSize(const zx::vmo& vmo) {
  size_t content_size = 45684651u;
  EXPECT_OK(vmo.get_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));
  return content_size;
}

TEST(StreamTestCase, WriteExtendsContentSize) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));
  size_t content_size = 3u;
  ASSERT_OK(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  zx::stream stream;
  char buffer[17] = "0123456789ABCDEF";
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = sizeof(buffer),
  };
  size_t actual = 42u;

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo, 0, &stream));
  vec.capacity = 7u;
  ASSERT_OK(stream.writev(0, &vec, 1, &actual));
  EXPECT_EQ(7u, actual);
  EXPECT_STREQ("0123456", GetData(vmo).c_str());
  EXPECT_EQ(7u, GetContentSize(vmo));
  ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));

  vec.capacity = 2u;
  ASSERT_OK(stream.writev(0, &vec, 1, &actual));
  EXPECT_EQ(2u, actual);
  EXPECT_STREQ("abcdefg01jklmnopqrstuvwxyz", GetData(vmo).c_str());
  EXPECT_EQ(9u, GetContentSize(vmo));
  ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));

  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_START, 0, nullptr));

  vec.capacity = 10u;
  for (size_t i = 1; i * 10 < zx_system_get_page_size(); ++i) {
    ASSERT_OK(stream.writev(0, &vec, 1, &actual));
    EXPECT_EQ(10u, actual);
  }
  EXPECT_EQ(4090u, GetContentSize(vmo));

  actual = 9823u;
  EXPECT_EQ(ZX_OK, stream.writev(0, &vec, 1, &actual));
  EXPECT_EQ(6u, actual);
  EXPECT_EQ(4096u, GetContentSize(vmo));

  char scratch[17] = {};
  ASSERT_OK(vmo.read(scratch, 4090u, 6u));
  EXPECT_STREQ("012345", scratch);

  actual = 9823u;
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, stream.writev(0, &vec, 1, &actual));
  EXPECT_EQ(4096u, GetContentSize(vmo));
}

TEST(StreamTestCase, WriteDoesntExtendVMOSize) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), ZX_VMO_RESIZABLE, &vmo));
  size_t content_size = 0u;
  ASSERT_OK(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  zx::stream stream;
  char buffer[17] = "0123456789ABCDEF";
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = sizeof(buffer),
  };
  size_t actual = 42u;

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo, 0, &stream));
  vec.capacity = 10u;
  for (size_t i = 1; i * 10 < zx_system_get_page_size(); ++i) {
    ASSERT_OK(stream.writev(0, &vec, 1, &actual));
    EXPECT_EQ(10u, actual);
  }
  EXPECT_EQ(4090u, GetContentSize(vmo));

  actual = 9823u;
  ASSERT_OK(stream.writev(0, &vec, 1, &actual));
  EXPECT_EQ(6u, actual);
  EXPECT_EQ(zx_system_get_page_size(), GetContentSize(vmo));

  // Assert that there is no remaining capacity in the underlying VMO by trying
  // to write one byte.
  vec.capacity = 1;
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, stream.writev(0, &vec, 1, nullptr));

  uint64_t vmo_size = 839u;
  ASSERT_OK(vmo.get_size(&vmo_size));
  EXPECT_EQ(zx_system_get_page_size(), vmo_size);

  vec.capacity = UINT64_MAX;
  actual = 5423u;
  ASSERT_EQ(ZX_ERR_FILE_BIG, stream.writev(0, &vec, 1, &actual));

  ASSERT_OK(vmo.get_size(&vmo_size));
  EXPECT_EQ(zx_system_get_page_size(), vmo_size);
}

TEST(StreamTestCase, ReadVAt) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));
  size_t content_size = 26u;
  ASSERT_OK(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  zx::stream stream;
  char buffer[16] = {};
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = sizeof(buffer),
  };
  size_t actual = 42u;

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ, vmo, 0, &stream));
  vec.capacity = 7u;
  ASSERT_OK(stream.readv_at(0, 24u, &vec, 1, &actual));
  EXPECT_EQ(2u, actual);
  EXPECT_STREQ("yz", buffer);
  memset(buffer, 0, sizeof(buffer));

  zx_off_t seek = 39u;
  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_CURRENT, 0, &seek));
  EXPECT_EQ(0u, seek);

  ASSERT_OK(stream.readv_at(0, 36u, &vec, 1, &actual));
  EXPECT_EQ(0u, actual);
  EXPECT_STREQ("", buffer);
  memset(buffer, 0, sizeof(buffer));

  ASSERT_OK(stream.readv_at(0, 3645651u, &vec, 1, &actual));
  EXPECT_EQ(0u, actual);
  EXPECT_STREQ("", buffer);
  memset(buffer, 0, sizeof(buffer));
}

TEST(StreamTestCase, WriteVAt) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));
  size_t content_size = 26u;
  ASSERT_OK(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  zx::stream stream;
  char buffer[17] = "0123456789ABCDEF";
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = sizeof(buffer),
  };
  size_t actual = 42u;

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo, 0, &stream));
  vec.capacity = 3u;
  ASSERT_OK(stream.writev_at(0, 7, &vec, 1, &actual));
  EXPECT_EQ(3u, actual);
  EXPECT_STREQ("abcdefg012klmnopqrstuvwxyz", GetData(vmo).c_str());
  ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));

  zx_off_t seek = 39u;
  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_CURRENT, 0, &seek));
  EXPECT_EQ(0u, seek);

  vec.capacity = 10u;
  actual = 9823u;
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, stream.writev_at(0, 4100u, &vec, 1, &actual));

  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), ZX_VMO_RESIZABLE, &vmo));
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo, 0, &stream));

  vec.capacity = 10u;
  actual = 9823u;
  ASSERT_OK(stream.writev_at(0, 4090, &vec, 1, &actual));
  EXPECT_EQ(6u, actual);
  EXPECT_EQ(zx_system_get_page_size(), GetContentSize(vmo));

  uint64_t vmo_size = 839u;
  ASSERT_OK(vmo.get_size(&vmo_size));
  EXPECT_EQ(zx_system_get_page_size(), vmo_size);

  vec.capacity = UINT64_MAX;
  actual = 5423u;
  ASSERT_EQ(ZX_ERR_FILE_BIG, stream.writev_at(0, 5414u, &vec, 1, &actual));

  // did not change the size of the VMO
  ASSERT_OK(vmo.get_size(&vmo_size));
  EXPECT_EQ(zx_system_get_page_size(), vmo_size);
  EXPECT_EQ(zx_system_get_page_size(), GetContentSize(vmo));

  zx_iovec_t bad_vec = {
      .buffer = nullptr,
      .capacity = 42u,
  };

  actual = 5423u;
  ASSERT_NOT_OK(stream.writev_at(0, 5000u, &bad_vec, 1, &actual));
  ASSERT_EQ(zx_system_get_page_size(), GetContentSize(vmo));
}

TEST(StreamTestCase, ReadVectorAlias) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));
  size_t content_size = 26u;
  ASSERT_OK(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  zx::stream stream;
  const size_t kVectorCount = 7;
  zx_iovec_t multivec[kVectorCount] = {};
  for (size_t i = 0; i < kVectorCount; ++i) {
    multivec[i].buffer = multivec;  // Notice the alias.
    multivec[i].capacity = sizeof(multivec);
  }

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ, vmo, 0, &stream));
  size_t actual = 42u;
  EXPECT_OK(stream.readv(0, multivec, kVectorCount, &actual));
  ASSERT_EQ(26u, actual);
}

TEST(StreamTestCase, Append) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));
  size_t content_size = 26u;
  ASSERT_OK(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  zx::stream stream;
  char buffer[17] = "0123456789ABCDEF";
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = sizeof(buffer),
  };

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo, 0, &stream));
  {
    zx_info_stream_t info{};
    ASSERT_OK(stream.get_info(ZX_INFO_STREAM, &info, sizeof(info), nullptr, nullptr));
    EXPECT_EQ(ZX_STREAM_MODE_WRITE, info.options);
    EXPECT_EQ(0u, info.seek);
    EXPECT_EQ(26u, info.content_size);
  }

  vec.capacity = 7u;
  size_t actual = 42u;
  ASSERT_OK(stream.writev(ZX_STREAM_APPEND, &vec, 1, &actual));
  EXPECT_EQ(7u, actual);
  EXPECT_STREQ("abcdefghijklmnopqrstuvwxyz0123456", GetData(vmo).c_str());

  {
    zx_info_stream_t info{};
    ASSERT_OK(stream.get_info(ZX_INFO_STREAM, &info, sizeof(info), nullptr, nullptr));
    EXPECT_EQ(ZX_STREAM_MODE_WRITE, info.options);
    EXPECT_EQ(33u, info.seek);
    EXPECT_EQ(33u, info.content_size);

    vec.capacity = 26u;
    for (size_t size = info.content_size; size + vec.capacity < zx_system_get_page_size();
         size += vec.capacity) {
      ASSERT_OK(stream.writev(ZX_STREAM_APPEND, &vec, 1, &actual));
      EXPECT_EQ(vec.capacity, actual);
    }
  }

  {
    zx_info_stream_t info{};
    ASSERT_OK(stream.get_info(ZX_INFO_STREAM, &info, sizeof(info), nullptr, nullptr));
    EXPECT_GT(zx_system_get_page_size(), info.content_size);

    EXPECT_OK(stream.writev(ZX_STREAM_APPEND, &vec, 1, &actual));
    EXPECT_EQ(zx_system_get_page_size() - info.content_size, actual);
  }

  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, stream.writev(ZX_STREAM_APPEND, &vec, 1, &actual));

  vec.capacity = UINT64_MAX;
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, stream.writev(ZX_STREAM_APPEND, &vec, 1, &actual));
}

TEST(StreamTestCase, WriteVectorWithStreamInAppendMode) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));
  size_t content_size = 26u;
  ASSERT_OK(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  zx::stream stream;
  char buffer[17] = "0123456789ABCDEF";
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = sizeof(buffer),
  };

  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND, vmo, 0, &stream));
  {
    zx_info_stream_t info{};
    ASSERT_OK(stream.get_info(ZX_INFO_STREAM, &info, sizeof(info), nullptr, nullptr));
    EXPECT_EQ(ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND, info.options);
    EXPECT_EQ(0u, info.seek);
    EXPECT_EQ(26u, info.content_size);
  }

  vec.capacity = 7u;
  size_t actual = 42u;
  ASSERT_OK(stream.writev(0, &vec, 1, &actual));
  EXPECT_EQ(7u, actual);
  EXPECT_STREQ("abcdefghijklmnopqrstuvwxyz0123456", GetData(vmo).c_str());

  {
    zx_info_stream_t info{};
    ASSERT_OK(stream.get_info(ZX_INFO_STREAM, &info, sizeof(info), nullptr, nullptr));
    EXPECT_EQ(ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND, info.options);
    EXPECT_EQ(33u, info.seek);
    EXPECT_EQ(33u, info.content_size);

    vec.capacity = 26u;
    for (size_t size = info.content_size; size + vec.capacity < zx_system_get_page_size();
         size += vec.capacity) {
      ASSERT_OK(stream.writev(0, &vec, 1, &actual));
      EXPECT_EQ(vec.capacity, actual);
    }
  }

  {
    zx_info_stream_t info{};
    ASSERT_OK(stream.get_info(ZX_INFO_STREAM, &info, sizeof(info), nullptr, nullptr));
    EXPECT_GT(zx_system_get_page_size(), info.content_size);

    ASSERT_OK(stream.writev(0, &vec, 1, &actual));
    EXPECT_EQ(zx_system_get_page_size() - info.content_size, actual);
  }

  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, stream.writev(0, &vec, 1, nullptr));

  vec.capacity = UINT64_MAX;
  ASSERT_EQ(ZX_ERR_OUT_OF_RANGE, stream.writev(0, &vec, 1, nullptr));
}

TEST(StreamTestCase, PropertyModeAppend) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ASSERT_OK(vmo.set_prop_content_size(0));

  zx::stream stream;
  char buffer[] = "0123456789ABCDEF";
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = 16,
  };

  // Create the stream not in append mode.
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo, 0, &stream));
  ASSERT_OK(stream.writev(0, &vec, 1, nullptr));

  {
    zx_info_stream_t info{};
    ASSERT_OK(stream.get_info(ZX_INFO_STREAM, &info, sizeof(info), nullptr, nullptr));
    EXPECT_FALSE(info.options & ZX_STREAM_MODE_APPEND);
    EXPECT_EQ(16u, info.seek);
    EXPECT_EQ(16u, info.content_size);
    uint8_t mode_append;
    ASSERT_OK(stream.get_prop_mode_append(&mode_append));
    EXPECT_FALSE(mode_append);
  }

  // Switch the stream to append mode.
  ASSERT_OK(stream.set_prop_mode_append(true));
  {
    zx_info_stream_t info{};
    ASSERT_OK(stream.get_info(ZX_INFO_STREAM, &info, sizeof(info), nullptr, nullptr));
    EXPECT_TRUE(info.options & ZX_STREAM_MODE_APPEND);
    uint8_t mode_append;
    ASSERT_OK(stream.get_prop_mode_append(&mode_append));
    EXPECT_TRUE(mode_append);
  }
  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_START, 10, nullptr));
  ASSERT_OK(stream.writev(0, &vec, 1, nullptr));
  EXPECT_STREQ("0123456789ABCDEF0123456789ABCDEF", GetData(vmo).c_str());

  // Take the stream out of append mode.
  ASSERT_OK(stream.set_prop_mode_append(false));
  {
    zx_info_stream_t info{};
    ASSERT_OK(stream.get_info(ZX_INFO_STREAM, &info, sizeof(info), nullptr, nullptr));
    EXPECT_FALSE(info.options & ZX_STREAM_MODE_APPEND);
    // The previous write appended to the stream despite the seek offset not being at the end of the
    // stream.
    EXPECT_EQ(32u, info.seek);
    EXPECT_EQ(32u, info.content_size);
    uint8_t mode_append;
    ASSERT_OK(stream.get_prop_mode_append(&mode_append));
    EXPECT_FALSE(mode_append);
  }
  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_START, 10, nullptr));
  ASSERT_OK(stream.writev(0, &vec, 1, nullptr));
  EXPECT_STREQ("01234567890123456789ABCDEFABCDEF", GetData(vmo).c_str());
}

TEST(StreamTestCase, AppendWithMultipleThreads) {
  // kThreadCount threads collectively write the numbers 0 to kBufferSize-1 to the vmo.
  constexpr uint64_t kThreadCount = 4;
  constexpr uint64_t kBufferSize = 256;
  constexpr uint64_t kIterationCount = kBufferSize / kThreadCount;
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ASSERT_OK(vmo.set_prop_content_size(0));

  std::vector<uint8_t> buffer(kBufferSize, 0);
  std::iota(buffer.begin(), buffer.end(), 0);

  std::vector<std::thread> threads;
  threads.reserve(kThreadCount);
  for (uint64_t thread = 0; thread < kThreadCount; ++thread) {
    threads.emplace_back([&vmo, &buffer, thread]() {
      zx::stream stream;
      ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND, vmo, 0, &stream));
      for (uint64_t i = 0; i < kIterationCount; ++i) {
        zx_iovec_t vec = {
            .buffer = &buffer[thread * kIterationCount + i],
            .capacity = 1,
        };
        ASSERT_OK(stream.writev(0, &vec, 1, nullptr));
      }
    });
  }
  for (auto& thread : threads) {
    thread.join();
  }

  // With several threads simultaneously appending, the data is likely out of order but none of the
  // appends should have overwritten each other.
  std::vector<uint8_t> vmo_data(kBufferSize, 0);
  ASSERT_OK(vmo.read(vmo_data.data(), 0, kBufferSize));
  std::sort(vmo_data.begin(), vmo_data.end());
  EXPECT_BYTES_EQ(buffer.data(), vmo_data.data(), kBufferSize);
}

TEST(StreamTestCase, ExtendFillsWithZeros) {
  const size_t kPageCount = 6;
  const size_t kVmoSize = zx_system_get_page_size() * kPageCount;
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));
  size_t content_size = 0u;
  ASSERT_OK(vmo.set_property(ZX_PROP_VMO_CONTENT_SIZE, &content_size, sizeof(content_size)));

  zx::stream stream;
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo, 0, &stream));

  char scratch[zx_system_get_page_size()];
  memset(scratch, 'x', sizeof(scratch));

  for (size_t i = 0; i < kPageCount; ++i) {
    ASSERT_OK(vmo.write(scratch, zx_system_get_page_size() * i, sizeof(scratch)));
  }

  char buffer[17] = "0123456789ABCDEF";
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = 4,
  };

  size_t actual = 0u;
  ASSERT_OK(stream.writev_at(0, zx_system_get_page_size() * 2 - 2, &vec, 1, &actual));
  ASSERT_EQ(4, actual);

  memset(scratch, 'a', sizeof(scratch));
  ASSERT_OK(vmo.read(scratch, 0, sizeof(scratch)));

  for (size_t i = 0; i < zx_system_get_page_size(); ++i) {
    ASSERT_EQ(0, scratch[i], "The %zu byte should be zero.", i);
  }

  memset(scratch, 'a', sizeof(scratch));
  ASSERT_OK(vmo.read(scratch, zx_system_get_page_size(), sizeof(scratch)));

  for (size_t i = 0; i < zx_system_get_page_size() - 2; ++i) {
    ASSERT_EQ(0, scratch[i], "The %zu byte of the second page should be zero.", i);
  }

  ASSERT_EQ('0', scratch[zx_system_get_page_size() - 2]);
  ASSERT_EQ('1', scratch[zx_system_get_page_size() - 1]);

  memset(scratch, 'a', sizeof(scratch));
  ASSERT_OK(vmo.read(scratch, zx_system_get_page_size() * 2, sizeof(scratch)));

  ASSERT_EQ('2', scratch[0]);
  ASSERT_EQ('3', scratch[1]);
  ASSERT_EQ('x', scratch[2]);
  ASSERT_EQ('x', scratch[3]);

  ASSERT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_START, zx_system_get_page_size() * 5 - 2, nullptr));

  actual = 0;
  ASSERT_OK(stream.writev(0, &vec, 1, &actual));
  ASSERT_EQ(4, actual);

  memset(scratch, 'a', sizeof(scratch));
  ASSERT_OK(vmo.read(scratch, zx_system_get_page_size() * 2, sizeof(scratch)));

  ASSERT_EQ('2', scratch[0]);
  ASSERT_EQ('3', scratch[1]);
  ASSERT_EQ(0, scratch[2]);
  ASSERT_EQ(0, scratch[3]);

  memset(scratch, 'a', sizeof(scratch));
  ASSERT_OK(vmo.read(scratch, zx_system_get_page_size() * 3, sizeof(scratch)));

  for (size_t i = 0; i < zx_system_get_page_size(); ++i) {
    ASSERT_EQ(0, scratch[i], "The %zu byte of the third page should be zero.", i);
  }

  memset(scratch, 'a', sizeof(scratch));
  ASSERT_OK(vmo.read(scratch, zx_system_get_page_size() * 4, sizeof(scratch)));

  for (size_t i = 0; i < zx_system_get_page_size() - 2; ++i) {
    ASSERT_EQ(0, scratch[i], "The %zu byte of the fourth page should be zero.", i);
  }

  ASSERT_EQ('0', scratch[zx_system_get_page_size() - 2]);
  ASSERT_EQ('1', scratch[zx_system_get_page_size() - 1]);

  memset(scratch, 'a', sizeof(scratch));
  ASSERT_OK(vmo.read(scratch, zx_system_get_page_size() * 5, sizeof(scratch)));

  ASSERT_EQ('2', scratch[0]);
  ASSERT_EQ('3', scratch[1]);
  ASSERT_EQ('x', scratch[2]);
  ASSERT_EQ('x', scratch[3]);
}

TEST(StreamTestCase, ReadShrinkRace) {
  constexpr size_t kNumIterations = 10;

  constexpr size_t kInitialVmoSize = 80u;
  constexpr size_t truncate_to_size = 0u;

  const size_t page_size = zx_system_get_page_size();
  const size_t initial_vmo_num_pages = fbl::round_up(kInitialVmoSize, page_size) / page_size;

  for (size_t i = 0; i < kNumIterations; ++i) {
    pager_tests::UserPager pager;
    ASSERT_TRUE(pager.Init());

    pager_tests::Vmo* vmo;
    ASSERT_TRUE(pager.CreateVmoWithOptions(initial_vmo_num_pages, ZX_VMO_RESIZABLE, &vmo));

    zx::stream stream;
    ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ, vmo->vmo(), 0, &stream));

    // Create a read that intersects with the truncate.
    std::thread read_thread([&] {
      std::array<char, 16u> buffer = {};
      zx_iovec_t vec = {
          .buffer = buffer.data(),
          .capacity = buffer.size(),
      };

      size_t actual = 42u;
      ASSERT_OK(stream.readv(0, &vec, 1, &actual));

      // The read should have happened either before or after the set size, so either nothing or
      // everything should've been read.
      ASSERT_TRUE(actual == 0u || actual == buffer.size());
    });

    std::thread set_size_thread([&] { ASSERT_OK(vmo->vmo().set_size(truncate_to_size)); });

    // Start a page fault handler to respond to read requests.
    vmo->SetPageFaultSupplyLimit(1);
    ASSERT_TRUE(pager.StartTaggedPageFaultHandler());

    set_size_thread.join();
    read_thread.join();

    // The set size must now be complete.
    uint64_t content_size = 42u;
    ASSERT_OK(vmo->vmo().get_prop_content_size(&content_size));
    EXPECT_EQ(truncate_to_size, content_size);

    // Reads should be okay and return nothing.
    std::array<char, 16u> buffer = {};
    zx_iovec_t vec = {
        .buffer = buffer.data(),
        .capacity = buffer.size(),
    };
    size_t actual = 42u;
    ASSERT_OK(stream.readv(0, &vec, 1, &actual));
    EXPECT_EQ(0u, actual);
  }
}

TEST(StreamTestCase, WriteShrinkRace) {
  constexpr size_t kNumIterations = 50;

  const size_t page_size = zx_system_get_page_size();
  const size_t initial_vmo_size = page_size + 8u;
  const size_t initial_vmo_num_pages = fbl::round_up(initial_vmo_size, page_size) / page_size;
  const size_t truncate_to_size = page_size;

  for (size_t i = 0; i < kNumIterations; ++i) {
    pager_tests::UserPager pager;
    ASSERT_TRUE(pager.Init());

    pager_tests::Vmo* vmo;
    ASSERT_TRUE(pager.CreateUnboundedVmo(0, 0, &vmo));

    zx::stream stream;
    ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo->vmo(), 0u, &stream));

    pager.SupplyPages(vmo, 0u, initial_vmo_num_pages);

    // Create a write that intersects with the truncate.
    std::thread boundary_write_thread([&] {
      std::array<char, 16u> buffer = {};
      ASSERT_LE(buffer.size(), initial_vmo_size);
      zx_iovec_t vec = {
          .buffer = buffer.data(),
          .capacity = buffer.size(),
      };

      // Attempt to write the last |buffer.size()| bytes.
      const zx_off_t kOffset = initial_vmo_size - buffer.size();
      size_t actual = 42u;
      ASSERT_OK(stream.writev_at(0, kOffset, &vec, 1u, &actual));
      ASSERT_EQ(actual, buffer.size());
    });

    // Create a write that should always complete, regardless of truncation.
    std::thread full_write_thread([&] {
      std::array<char, 16> buffer = {};
      ASSERT_LE(buffer.size(), initial_vmo_size);
      zx_iovec_t vec = {
          .buffer = buffer.data(),
          .capacity = buffer.size(),
      };

      // Attempt to write the first |buffer.size()| bytes.
      size_t actual = 42u;
      ASSERT_OK(stream.writev_at(0, 0u, &vec, 1u, &actual));
      ASSERT_EQ(actual, buffer.size());
    });

    // Simultaneously try to truncate.
    std::thread truncate_thread([&] { ASSERT_OK(vmo->vmo().set_stream_size(truncate_to_size)); });

    boundary_write_thread.join();
    full_write_thread.join();
    truncate_thread.join();

    // The set size must now be complete.
    // The size will either be |truncate_to_size| if the truncate happened last or
    // |initial_vmo_size| if the write happened last.
    uint64_t content_size = 42u;
    ASSERT_OK(vmo->vmo().get_prop_content_size(&content_size));
    ASSERT_TRUE(content_size == initial_vmo_size || content_size == truncate_to_size);
  }
}

TEST(StreamTestCase, ReadWriteShrinkRace) {
  constexpr size_t kNumIterations = 500;

  const size_t page_size = zx_system_get_page_size();
  const size_t initial_vmo_size = 8 * page_size + 8u;
  const size_t initial_vmo_num_pages = fbl::round_up(initial_vmo_size, page_size) / page_size;
  const size_t truncate_to_size = page_size;

  for (size_t i = 0; i < kNumIterations; ++i) {
    pager_tests::UserPager pager;
    ASSERT_TRUE(pager.Init());

    pager_tests::Vmo* vmo;
    ASSERT_TRUE(pager.CreateVmoWithOptions(initial_vmo_num_pages, ZX_VMO_RESIZABLE, &vmo));

    zx::stream stream;
    ASSERT_OK(
        zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, vmo->vmo(), 0u, &stream));

    pager.SupplyPages(vmo, 0u, initial_vmo_num_pages);

    // Create a write that intersects with the truncate.
    std::thread write_thread([&] {
      std::array<char, 16u> buffer = {};
      ASSERT_LE(buffer.size(), initial_vmo_size);
      zx_iovec_t vec = {
          .buffer = buffer.data(),
          .capacity = buffer.size(),
      };

      // Attempt to write the last |buffer.size()| bytes.
      const zx_off_t offset = initial_vmo_size - buffer.size();
      size_t actual = 42u;
      ASSERT_OK(stream.writev_at(0, offset, &vec, 1u, &actual));
      ASSERT_EQ(actual, buffer.size());
    });

    // Simultaneously try to truncate.
    std::thread truncate_thread([&] { ASSERT_OK(vmo->vmo().set_stream_size(truncate_to_size)); });

    // Create a read that intersects with the truncate.
    std::thread read_thread([&] {
      std::vector<char> buffer(initial_vmo_size, 0);
      zx_iovec_t vec = {
          .buffer = buffer.data(),
          .capacity = buffer.size(),
      };

      size_t actual = 42u;
      ASSERT_OK(stream.readv_at(0, 0u, &vec, 1u, &actual));
      // If the write happens after the truncate, the read may see a content size in the range
      // [truncate_to_size, initial_vmo_size] because of a partial expanding write updating content
      // size as it progresses.
      ASSERT_TRUE(actual >= truncate_to_size || actual <= initial_vmo_size);
    });

    write_thread.join();
    truncate_thread.join();
    read_thread.join();

    // The set size must now be complete.
    // The size will either be |truncate_to_size| if the truncate happened last or
    // |initial_vmo_size| if the write happened last.
    uint64_t content_size = 42u;
    ASSERT_OK(vmo->vmo().get_prop_content_size(&content_size));
    ASSERT_TRUE(content_size == initial_vmo_size || content_size == truncate_to_size);
  }
}

// Regression test for https://fxbug.dev/42176351. Writing to an offset that requires expansion
// should not result in an overflow when computing the new required VMO size.
TEST(StreamTestCase, ExpandOverflow) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), ZX_VMO_RESIZABLE, &vmo));

  zx::stream stream;
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo, 0, &stream));

  char buffer[] = "AAAA";
  zx_iovec_t vec = {
      .buffer = buffer,
      .capacity = 4,
  };

  size_t actual = 0u;
  // This write will require a content size of 0xfffffffffffffffc, which when rounded up to the page
  // boundary to compute the VMO size will overflow. So content expansion should fail.
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, stream.writev_at(0, 0xfffffffffffffff8, &vec, 1, &actual));
  EXPECT_EQ(0, actual);

  // Verify the VMO and content sizes.
  uint64_t vmo_size;
  ASSERT_OK(vmo.get_size(&vmo_size));
  EXPECT_EQ(zx_system_get_page_size(), vmo_size);

  uint64_t content_size;
  ASSERT_OK(vmo.get_prop_content_size(&content_size));
  EXPECT_EQ(zx_system_get_page_size(), content_size);

  // Verify that a subsequent resize succeeds.
  EXPECT_OK(vmo.set_size(2 * zx_system_get_page_size()));
  ASSERT_OK(vmo.get_size(&vmo_size));
  EXPECT_EQ(2 * zx_system_get_page_size(), vmo_size);
  ASSERT_OK(vmo.get_prop_content_size(&content_size));
  EXPECT_EQ(2 * zx_system_get_page_size(), content_size);
}

// Tests that content size is updated as soon as bytes are committed to the VMO.
TEST(StreamTestCase, ContentSizeUpdatedOnPartialWrite) {
  constexpr uint64_t kNumPagesToWrite = kMaxPagesBatch * 3;

  pager_tests::UserPager pager;
  ASSERT_TRUE(pager.Init());

  pager_tests::Vmo* vmo;
  ASSERT_TRUE(pager.CreateUnboundedVmo(0, ZX_VMO_TRAP_DIRTY, &vmo));

  zx::stream stream;
  ASSERT_OK(
      zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, vmo->vmo(), 0u, &stream));

  std::thread write_thread([&] {
    std::vector<char> buffer(kNumPagesToWrite * zx_system_get_page_size(), 'a');
    zx_iovec_t vec = {
        .buffer = buffer.data(),
        .capacity = buffer.size(),
    };
    size_t actual;
    ASSERT_OK(stream.writev(0, &vec, 1, &actual));

    ASSERT_EQ(actual, buffer.size());
  });

  for (uint64_t page_num = 0; page_num < kNumPagesToWrite; page_num += kMaxPagesBatch) {
    const uint64_t num_pages_to_dirty = std::min(kMaxPagesBatch, kNumPagesToWrite - page_num);

    pager.WaitForPageDirty(vmo, page_num, num_pages_to_dirty, ZX_TIME_INFINITE);
    ASSERT_EQ(GetContentSize(vmo->vmo()), page_num * zx_system_get_page_size());
    pager.DirtyPages(vmo, page_num, num_pages_to_dirty);
  }

  write_thread.join();
}

// Tests that resizing a `zx_iovec_t` capacity smaller while a read is using it does not fail.
TEST(StreamTestCase, RaceReadResizeVecSmaller) {
  constexpr size_t kNumIterations = 50;
  constexpr size_t kInitialVecSize = 26;
  constexpr size_t kResizeVecSize = 10;
  constexpr char kInitialBufferChar = '!';

  for (size_t i = 0; i < kNumIterations; ++i) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
    ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));

    std::string buffer(kInitialVecSize, kInitialBufferChar);
    zx_iovec_t vec = {
        .buffer = buffer.data(),
        .capacity = buffer.size(),
    };

    zx::stream stream;
    ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ, vmo, 0, &stream));

    std::thread read_thread([&] {
      size_t actual = 42u;
      ASSERT_OK(stream.readv(0, &vec, 1, &actual));

      ASSERT_TRUE(actual == buffer.size() || actual == kResizeVecSize);

      if (actual == kResizeVecSize) {
        std::string spliced = std::string(kAlphabet).substr(0, kResizeVecSize) +
                              std::string(kInitialVecSize - kResizeVecSize, kInitialBufferChar);

        EXPECT_STREQ(spliced.c_str(), buffer.c_str());
      } else {
        EXPECT_STREQ(kAlphabet, GetData(vmo).c_str());
      }
    });

    std::thread resize_thread([&] { vec.capacity = kResizeVecSize; });

    read_thread.join();
    resize_thread.join();
  }
}

// Tests that resizing a `zx_iovec_t` capacity smaller while a write is using it does not fail.
TEST(StreamTestCase, RaceWriteResizeVecSmaller) {
  constexpr size_t kNumIterations = 50;
  constexpr size_t kResizeVecSize = 10;

  for (size_t i = 0; i < kNumIterations; ++i) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
    ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));

    std::string buffer = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    zx_iovec_t vec = {
        .buffer = buffer.data(),
        .capacity = buffer.size(),
    };

    zx::stream stream;
    ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo, 0, &stream));

    std::thread write_thread([&] {
      size_t actual = 42u;
      ASSERT_OK(stream.writev(0, &vec, 1, &actual));

      ASSERT_TRUE(actual == buffer.size() || actual == kResizeVecSize);

      if (actual == kResizeVecSize) {
        std::string spliced =
            buffer.substr(0, kResizeVecSize) + std::string(kAlphabet).substr(kResizeVecSize);

        EXPECT_STREQ(spliced.c_str(), GetData(vmo).c_str());
      } else {
        EXPECT_STREQ(buffer.c_str(), GetData(vmo).c_str());
      }
    });

    std::thread resize_thread([&] { vec.capacity = kResizeVecSize; });

    write_thread.join();
    resize_thread.join();
  }
}

// Tests that resizing a `zx_iovec_t` capacity larger while a read is using it does not fail.
TEST(StreamTestCase, RaceReadResizeVecLarger) {
  constexpr size_t kNumIterations = 50;
  constexpr size_t kInitialVecSize = 10;
  constexpr size_t kResizeVecSize = 26;
  constexpr char kInitialBufferChar = '!';

  for (size_t i = 0; i < kNumIterations; ++i) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
    ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));

    std::string buffer(kResizeVecSize, kInitialBufferChar);
    zx_iovec_t vec = {
        .buffer = buffer.data(),
        .capacity = kInitialVecSize,
    };

    zx::stream stream;
    ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ, vmo, 0, &stream));

    std::thread read_thread([&] {
      size_t actual = 42u;
      ASSERT_OK(stream.readv(0, &vec, 1, &actual));

      ASSERT_TRUE(actual == kInitialVecSize || actual == buffer.size());

      if (actual == kResizeVecSize) {
        EXPECT_STREQ(kAlphabet, buffer.c_str());
      } else {
        std::string spliced = std::string(kAlphabet).substr(0, kInitialVecSize) +
                              std::string(kResizeVecSize - kInitialVecSize, kInitialBufferChar);

        EXPECT_STREQ(spliced.c_str(), buffer.c_str());
      }
    });

    std::thread resize_thread([&] { vec.capacity = kResizeVecSize; });

    read_thread.join();
    resize_thread.join();
  }
}

// Tests that resizing a `zx_iovec_t` capacity larger while a write is using it does not fail.
TEST(StreamTestCase, RaceWriteResizeVecLarger) {
  constexpr size_t kNumIterations = 50;
  constexpr size_t kInitialVecSize = 10;

  for (size_t i = 0; i < kNumIterations; ++i) {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
    ASSERT_OK(vmo.write(kAlphabet, 0u, strlen(kAlphabet)));

    std::string buffer = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    zx_iovec_t vec = {
        .buffer = buffer.data(),
        .capacity = kInitialVecSize,
    };

    zx::stream stream;
    ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo, 0, &stream));

    std::thread write_thread([&] {
      size_t actual = 42u;
      ASSERT_OK(stream.writev(0, &vec, 1, &actual));

      ASSERT_TRUE(actual == kInitialVecSize || actual == buffer.size());

      if (actual == kInitialVecSize) {
        std::string spliced =
            buffer.substr(0, kInitialVecSize) + std::string(kAlphabet).substr(kInitialVecSize);

        EXPECT_STREQ(spliced.c_str(), GetData(vmo).c_str());
      } else {
        EXPECT_STREQ(buffer.c_str(), GetData(vmo).c_str());
      }
    });

    std::thread resize_thread([&] { vec.capacity = buffer.size(); });

    write_thread.join();
    resize_thread.join();
  }
}

TEST(StreamTestCase, PartialVmoDirty) {
  // The created VMO has to have at least 1 page because it gets mapped. The 2nd and 3rd pages are
  // written to by the stream. The pager allows the 2nd page to be dirtied but fails the 3rd page
  // with ZX_ERR_NO_SPACE. The stream write call should succeed with 1 page being written.
  const size_t page_size = zx_system_get_page_size();
  pager_tests::UserPager pager;
  ASSERT_TRUE(pager.Init());
  pager_tests::Vmo* vmo;
  ASSERT_TRUE(pager.CreateUnboundedVmo(0, ZX_VMO_TRAP_DIRTY, &vmo));
  zx::stream stream;
  ASSERT_OK(
      zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, vmo->vmo(), 0u, &stream));

  std::thread pager_thread([&pager, vmo]() {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 2, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 1, 1));
    ASSERT_TRUE(pager.FailPages(vmo, 2, 1, ZX_ERR_NO_SPACE));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.FailPages(vmo, 2, 1, ZX_ERR_NO_SPACE));
  });

  auto buffer = std::make_unique<uint8_t[]>(page_size * 2);
  zx_iovec_t iovec = {
      .buffer = buffer.get(),
      .capacity = page_size * 2,
  };
  size_t bytes_written = 0;
  ASSERT_OK(stream.writev_at(0, page_size, &iovec, 1, &bytes_written));
  ASSERT_EQ(page_size, bytes_written);

  pager_thread.join();
}

TEST(StreamTestCase, AppendSuppliesZeroes) {
  const size_t page_size = zx_system_get_page_size();
  pager_tests::UserPager pager;
  ASSERT_TRUE(pager.Init());
  pager_tests::Vmo* vmo;
  ASSERT_TRUE(pager.CreateUnboundedVmo(0, ZX_VMO_TRAP_DIRTY, &vmo));
  zx::stream stream;
  ASSERT_OK(
      zx::stream::create(ZX_STREAM_MODE_WRITE | ZX_STREAM_MODE_APPEND, vmo->vmo(), 0u, &stream));

  const size_t kAppends[] = {page_size / 2, page_size,     page_size * 2,
                             page_size / 2, page_size * 2, page_size / 4,
                             page_size / 4, page_size / 4, page_size};

  std::thread pager_thread([&]() {
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 0, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 0, 1));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 1, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 1, 1));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 2, 2, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 2, 2));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 4, 2, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 4, 2));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 6, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 6, 1));
    ASSERT_TRUE(pager.WaitForPageDirty(vmo, 7, 1, ZX_TIME_INFINITE));
    ASSERT_TRUE(pager.DirtyPages(vmo, 7, 1));
  });

  auto buffer = std::make_unique<uint8_t[]>(page_size * 2);
  for (size_t size : kAppends) {
    zx_iovec_t iovec = {
        .buffer = buffer.get(),
        .capacity = size,
    };
    size_t bytes_written = 0;
    EXPECT_OK(stream.writev(ZX_STREAM_APPEND, &iovec, 1, &bytes_written));
    EXPECT_EQ(bytes_written, size);
  }
  uint64_t offset, length;
  // No more requests.
  ASSERT_FALSE(pager.GetPageDirtyRequest(vmo, 0, &offset, &length));
  // If a read request had been generated this test would have hung, but validate there are none
  // anyway.
  ASSERT_FALSE(pager.GetPageReadRequest(vmo, 0, &offset, &length));
  pager_thread.join();
}

// Tests that passing a contiguous or physical VMO into zx_stream_create will return
// ZX_ERR_WRONG_TYPE. Also test stream operations on physical or contiguous VMOs.
TEST(StreamTestCase, NoStreamFromContiguousOrPhysicalVmo) {
  // Resources for contiguous VMO.
  zx::unowned_resource system_resource = maybe_standalone::GetSystemResource();
  if (!system_resource->is_valid()) {
    printf("System resource not available, skipping\n");
    return;
  }

  zx::result<zx::resource> result =
      maybe_standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_IOMMU_BASE);
  ASSERT_OK(result.status_value());
  zx::resource iommu_resource = std::move(result.value());

  zx::iommu iommu;
  zx::bti bti;
  zx_iommu_desc_stub_t desc;
  auto final_bti_check = fit::defer([&bti]() {
    if (bti.is_valid()) {
      zx_info_bti_t info;
      ASSERT_OK(bti.get_info(ZX_INFO_BTI, &info, sizeof(info), nullptr, nullptr));
      EXPECT_EQ(0, info.pmo_count);
      EXPECT_EQ(0, info.quarantine_count);
    }
  });

  ASSERT_OK(zx::iommu::create(iommu_resource, ZX_IOMMU_TYPE_STUB, &desc, sizeof(desc), &iommu));
  EXPECT_OK(zx::bti::create(iommu, 0, 0xdead1eaf, &bti));

  size_t vmo_size = zx_system_get_page_size();

  // Create a contiguous VMO & check that a stream cannot be created.
  zx::vmo contig_vmo;
  ASSERT_OK(zx::vmo::create_contiguous(bti, vmo_size, 0, &contig_vmo));

  zx::stream stream;
  EXPECT_EQ(ZX_ERR_WRONG_TYPE, zx::stream::create(ZX_STREAM_MODE_READ, contig_vmo, 0, &stream));

  uint64_t stream_size = 0;
  uint64_t content_size = 0;

  // Stream size on contiguous VMO is initialised to 0 and cannot be modified
  EXPECT_OK(contig_vmo.get_stream_size(&stream_size));
  EXPECT_EQ(0, stream_size);
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, contig_vmo.set_stream_size(42));
  EXPECT_EQ(0, stream_size);

  // Legacy prop_content_size should not be able to modify the stream size.
  EXPECT_OK(contig_vmo.get_prop_content_size(&content_size));
  EXPECT_EQ(0, content_size);
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, contig_vmo.set_prop_content_size(42));
  EXPECT_EQ(0, content_size);

  // Create a physical VMO & check that a stream cannot be created.
  vmo_test::PhysVmo phys_vmo;
  if (auto res = vmo_test::GetTestPhysVmo(vmo_size); !res.is_ok()) {
    if (res.error_value() == ZX_ERR_NOT_SUPPORTED) {
      printf("Root resource not available, skipping\n");
    }
    return;
  } else {
    phys_vmo = std::move(res.value());
  }

  // Stream size on contiguous VMO is initialised to 0 and cannot be modified.
  EXPECT_OK(phys_vmo.vmo.get_stream_size(&stream_size));
  EXPECT_EQ(0, stream_size);
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, phys_vmo.vmo.set_stream_size(42));
  EXPECT_EQ(0, stream_size);

  // Legacy prop_content_size should not be able to modify the stream size.
  EXPECT_OK(contig_vmo.get_prop_content_size(&content_size));
  EXPECT_EQ(0, content_size);
  EXPECT_EQ(ZX_ERR_NOT_SUPPORTED, contig_vmo.set_prop_content_size(42));
  EXPECT_EQ(0, content_size);
}

// Tests that when reducing the stream size, the range between the old and new stream size can not
// have a mapping snuck in, which would allow memory accesses past the stream size without a fault
// in a FAULT_BEYOND_STREAM_SIZE mapping.
TEST(StreamTestCase, FaultBeyondStreamSizeResizeDownRace) {
  constexpr size_t kNumIterations = 100;

  constexpr size_t kTestPages = 8;
  const size_t kPageSize = zx_system_get_page_size();
  const size_t kVmoSize = kTestPages * kPageSize;
  const size_t kTestOffset = kVmoSize / 2;

  zx::vmo vmo;

  ASSERT_OK(zx::vmo::create(kVmoSize, 0, &vmo));

  zx_vaddr_t vaddr;
  ASSERT_OK(zx::vmar::root_self()->map(
      ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_ALLOW_FAULTS | ZX_VM_FAULT_BEYOND_STREAM_SIZE, 0,
      vmo, 0, kVmoSize, &vaddr));

  auto unmap = fit::defer([&]() { zx::vmar::root_self()->unmap(vaddr, kVmoSize); });

  for (size_t i = 0; i < kNumIterations; i++) {
    vmo.set_stream_size(kVmoSize);
    EXPECT_OK(vmo.write(&vaddr, kTestOffset, sizeof(vaddr)));

    pager_tests::TestThread set_thread([&]() -> bool {
      EXPECT_OK(vmo.write(&vaddr, kTestOffset, sizeof(vaddr)));
      EXPECT_OK(vmo.set_stream_size(0));
      return true;
    });

    std::atomic<bool> terminate = false;
    pager_tests::TestThread read_spam([&]() -> bool {
      while (!terminate) {
        volatile uint64_t* test_addr = reinterpret_cast<volatile uint64_t*>(vaddr + kTestOffset);
        if (*test_addr == 42) {
          return true;
        }
      }
      return false;
    });

    pager_tests::TestThread fault_thread([&]() -> bool {
      volatile uint64_t* test_addr = reinterpret_cast<volatile uint64_t*>(vaddr + kTestOffset);
      if (*test_addr != 0) {
        return true;
      }
      return true;
    });

    auto terminate_read_spam = fit::defer([&terminate] { terminate = true; });

    // Interleave read & reduce stream size.
    ASSERT_TRUE(read_spam.Start());
    ASSERT_TRUE(set_thread.Start());

    // Let read & resize finish.
    ASSERT_TRUE(set_thread.Wait());
    terminate = true;
    read_spam.WaitForAnyCrash();

    // A read now should fault.
    ASSERT_TRUE(fault_thread.Start());
    ASSERT_TRUE(fault_thread.WaitForCrash(vaddr + kTestOffset, ZX_ERR_OUT_OF_RANGE));
  }
}

TEST(StreamTestCase, SyscallInvalidArgs) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  zx_handle_t raw_stream = ZX_HANDLE_INVALID;
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_stream_create(0xFFFFFFFF, vmo.get(), 0, &raw_stream));
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_stream_create(0, ZX_HANDLE_INVALID, 0, &raw_stream));

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  EXPECT_EQ(ZX_ERR_WRONG_TYPE, zx_stream_create(0, event.get(), 0, &raw_stream));

  zx::vmo read_only_vmo;
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_READ, &read_only_vmo));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED,
            zx_stream_create(ZX_STREAM_MODE_WRITE, read_only_vmo.get(), 0, &raw_stream));

  zx::vmo write_only_vmo;
  ASSERT_OK(vmo.duplicate(ZX_RIGHT_WRITE, &write_only_vmo));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED,
            zx_stream_create(ZX_STREAM_MODE_READ, write_only_vmo.get(), 0, &raw_stream));

  zx::stream rw_stream;
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, vmo, 0, &rw_stream));

  zx::stream ro_stream;
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ, vmo, 0, &ro_stream));

  zx::stream wo_stream;
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo, 0, &wo_stream));

  char buf[8] = {};
  zx_iovec_t vec = {
      .buffer = buf,
      .capacity = sizeof(buf),
  };
  size_t actual = 0;

  // zx_stream_writev
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_stream_writev(rw_stream.get(), 0x1234, &vec, 1, &actual));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_stream_writev(rw_stream.get(), 0, nullptr, 1, &actual));
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_stream_writev(ZX_HANDLE_INVALID, 0, &vec, 1, &actual));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, zx_stream_writev(ro_stream.get(), 0, &vec, 1, &actual));

  // zx_stream_writev_at
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_stream_writev_at(rw_stream.get(), 1, 0, &vec, 1, &actual));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_stream_writev_at(rw_stream.get(), 0, 0, nullptr, 1, &actual));
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_stream_writev_at(ZX_HANDLE_INVALID, 0, 0, &vec, 1, &actual));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, zx_stream_writev_at(ro_stream.get(), 0, 0, &vec, 1, &actual));

  // zx_stream_readv
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_stream_readv(rw_stream.get(), 1, &vec, 1, &actual));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_stream_readv(rw_stream.get(), 0, nullptr, 1, &actual));
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_stream_readv(ZX_HANDLE_INVALID, 0, &vec, 1, &actual));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, zx_stream_readv(wo_stream.get(), 0, &vec, 1, &actual));

  // zx_stream_readv_at
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_stream_readv_at(rw_stream.get(), 1, 0, &vec, 1, &actual));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_stream_readv_at(rw_stream.get(), 0, 0, nullptr, 1, &actual));
  EXPECT_EQ(ZX_ERR_BAD_HANDLE, zx_stream_readv_at(ZX_HANDLE_INVALID, 0, 0, &vec, 1, &actual));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED, zx_stream_readv_at(wo_stream.get(), 0, 0, &vec, 1, &actual));

  // zx_stream_seek
  zx_off_t seek_out = 0;
  EXPECT_EQ(ZX_ERR_BAD_HANDLE,
            zx_stream_seek(ZX_HANDLE_INVALID, ZX_STREAM_SEEK_ORIGIN_START, 0, &seek_out));

  zx::stream no_rights_stream;
  ASSERT_OK(rw_stream.duplicate(0, &no_rights_stream));
  EXPECT_EQ(ZX_ERR_ACCESS_DENIED,
            zx_stream_seek(no_rights_stream.get(), ZX_STREAM_SEEK_ORIGIN_START, 0, &seek_out));
  EXPECT_EQ(
      ZX_ERR_INVALID_ARGS,
      zx_stream_seek(rw_stream.get(), static_cast<zx_stream_seek_origin_t>(999), 0, &seek_out));
}

TEST(StreamTestCase, ZeroLengthVectors) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ASSERT_OK(vmo.set_prop_content_size(100));

  zx::stream stream;
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, vmo, 10, &stream));

  char buf[8] = {};
  zx_iovec_t zero_vec = {
      .buffer = buf,
      .capacity = 0,
  };
  size_t actual = 999;

  // With vector_count = 0 and nullptr vector
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_stream_readv(stream.get(), 0, nullptr, 0, &actual));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_stream_readv_at(stream.get(), 0, 0, nullptr, 0, &actual));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_stream_writev(stream.get(), 0, nullptr, 0, &actual));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, zx_stream_writev_at(stream.get(), 0, 0, nullptr, 0, &actual));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS,
            zx_stream_writev(stream.get(), ZX_STREAM_APPEND, nullptr, 0, &actual));

  // With vector_count = 1 and capacity = 0
  actual = 999;
  EXPECT_OK(zx_stream_readv(stream.get(), 0, &zero_vec, 1, &actual));
  EXPECT_EQ(0u, actual);

  actual = 999;
  EXPECT_OK(zx_stream_readv_at(stream.get(), 0, 5, &zero_vec, 1, &actual));
  EXPECT_EQ(0u, actual);

  actual = 999;
  EXPECT_OK(zx_stream_writev(stream.get(), 0, &zero_vec, 1, &actual));
  EXPECT_EQ(0u, actual);

  actual = 999;
  EXPECT_OK(zx_stream_writev_at(stream.get(), 0, 5, &zero_vec, 1, &actual));
  EXPECT_EQ(0u, actual);

  actual = 999;
  EXPECT_OK(zx_stream_writev(stream.get(), ZX_STREAM_APPEND, &zero_vec, 1, &actual));
  EXPECT_EQ(0u, actual);

  // Verify seek position has not changed
  zx_off_t seek_pos = 0;
  EXPECT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_CURRENT, 0, &seek_pos));
  EXPECT_EQ(10u, seek_pos);
}

TEST(StreamTestCase, GetInfoReadOnly) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));
  ASSERT_OK(vmo.set_prop_content_size(100));

  zx::stream ro_stream;
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ, vmo, 15, &ro_stream));

  zx_info_stream_t info = {};
  ASSERT_OK(ro_stream.get_info(ZX_INFO_STREAM, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(ZX_STREAM_MODE_READ, info.options);
  EXPECT_EQ(15u, info.seek);
  EXPECT_EQ(100u, info.content_size);
}

TEST(StreamTestCase, VectorCapacityOverflow) {
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  zx::stream stream;
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, vmo, 0, &stream));

  char buf[8] = {};
  size_t actual = 0;

  // Multi-vector capacity sum overflow in GetTotalCapacity (returns ZX_ERR_INVALID_ARGS)
  zx_iovec_t bad_overflow_vec[2] = {
      {.buffer = buf, .capacity = SIZE_MAX},
      {.buffer = buf, .capacity = 1},
  };
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, stream.readv(0, bad_overflow_vec, 2, &actual));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, stream.readv_at(0, 0, bad_overflow_vec, 2, &actual));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, stream.writev(0, bad_overflow_vec, 2, &actual));
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, stream.writev_at(0, 0, bad_overflow_vec, 2, &actual));

  // Offset + capacity overflow in CreateWriteOp (returns ZX_ERR_FILE_BIG)
  zx_iovec_t overflow_vec = {
      .buffer = buf,
      .capacity = UINT64_MAX,
  };
  EXPECT_EQ(ZX_ERR_FILE_BIG, stream.writev_at(0, 100, &overflow_vec, 1, &actual));
}

TEST(StreamTestCase, PartialAppendAtVmoBoundary) {
  const size_t page_size = zx_system_get_page_size();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(page_size, 0, &vmo));
  ASSERT_OK(vmo.set_prop_content_size(page_size - 4));

  zx::stream stream;
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_WRITE, vmo, 0, &stream));

  char buf[] = "0123456789";
  zx_iovec_t vec = {
      .buffer = buf,
      .capacity = 10,
  };
  size_t actual = 0;

  // Partial append: requested 10 bytes, but non-resizable VMO only has 4 bytes capacity left.
  // Shrinks the operation size to available capacity and commits 4 bytes.
  ASSERT_OK(stream.writev(ZX_STREAM_APPEND, &vec, 1, &actual));
  EXPECT_EQ(4u, actual);
  EXPECT_EQ(page_size, GetContentSize(vmo));

  // Verify seek position advanced to the end of the partial append.
  zx_off_t seek_pos = 0;
  EXPECT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_CURRENT, 0, &seek_pos));
  EXPECT_EQ(page_size, seek_pos);

  // When VMO is completely full, append returns ZX_ERR_OUT_OF_RANGE.
  EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, stream.writev(ZX_STREAM_APPEND, &vec, 1, &actual));
}

TEST(StreamTestCase, WriteVectorUnmappedUserBufferCancelsOperation) {
  const size_t page_size = zx_system_get_page_size();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(page_size, 0, &vmo));
  ASSERT_OK(vmo.set_prop_content_size(0));

  zx::stream stream;
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, vmo, 0, &stream));

  zx::vmar child_vmar;
  zx_vaddr_t child_addr = 0;
  ASSERT_OK(zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0, page_size * 2,
      &child_vmar, &child_addr));
  auto destroy_vmar = fit::defer([&]() { child_vmar.destroy(); });

  zx::vmo user_vmo;
  ASSERT_OK(zx::vmo::create(page_size, 0, &user_vmo));

  zx_vaddr_t mapped_addr = 0;
  ASSERT_OK(child_vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 0, user_vmo, 0,
                           page_size, &mapped_addr));

  // Buffer starts 4 bytes before end of mapped page, crossing into unmapped page.
  zx_iovec_t partial_user_vec = {
      .buffer = reinterpret_cast<void*>(mapped_addr + page_size - 4),
      .capacity = 8,
  };
  size_t actual = 0;

  // Failing user copy cancels the pending stream size operation and returns ZX_ERR_NOT_FOUND.
  EXPECT_EQ(ZX_ERR_NOT_FOUND, stream.writev(0, &partial_user_vec, 1, &actual));
  EXPECT_EQ(ZX_ERR_NOT_FOUND, stream.writev_at(0, 0, &partial_user_vec, 1, &actual));
  EXPECT_EQ(ZX_ERR_NOT_FOUND, stream.writev(ZX_STREAM_APPEND, &partial_user_vec, 1, &actual));

  // Verify stream seek and content size were not modified by the cancelled operations.
  EXPECT_EQ(0u, GetContentSize(vmo));
  zx_off_t seek_pos = 0;
  EXPECT_OK(stream.seek(ZX_STREAM_SEEK_ORIGIN_CURRENT, 0, &seek_pos));
  EXPECT_EQ(0u, seek_pos);
}

}  // namespace
