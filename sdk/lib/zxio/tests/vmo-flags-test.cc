// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/zx/channel.h>
#include <lib/zx/vmo.h>
#include <lib/zxio/cpp/create_with_type.h>
#include <lib/zxio/zxio.h>
#include <limits.h>
#include <zircon/compiler.h>

#include <zxtest/zxtest.h>

namespace {

constexpr size_t kSize = 300;
constexpr zx_off_t kInitialSeek = 4;

constexpr const char* ALPHABET = "abcdefghijklmnopqrstuvwxyz";

void GetVmo(zx::vmo& backing) {
  ASSERT_OK(zx::vmo::create(kSize, 0u, &backing));
  size_t len = strlen(ALPHABET);
  ASSERT_OK(backing.write(ALPHABET, 0, len));
  ASSERT_OK(backing.write(ALPHABET, len, len + len));
}

TEST(VmoTest, FlagsGetReadWrite) {
  zx::vmo backing;
  ASSERT_NO_FAILURES(GetVmo(backing));
  zx::stream stream;
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ | ZX_STREAM_MODE_WRITE, backing, kInitialSeek,
                               &stream));
  zxio_storage_t storage;
  ASSERT_OK(zxio::CreateVmo(&storage, std::move(backing), std::move(stream)));
  zxio_t* io = &storage.io;

  uint64_t raw_flags{};
  ASSERT_STATUS(ZX_OK, zxio_flags_get(io, &raw_flags));
  fuchsia_io::wire::Flags flags{raw_flags};
  EXPECT_TRUE(flags & fuchsia_io::wire ::Flags::kPermReadBytes);
  EXPECT_TRUE(flags & fuchsia_io::wire ::Flags::kPermWriteBytes);
  EXPECT_FALSE(flags & fuchsia_io::wire::Flags::kPermExecute);

  zxio_destroy(io);
}

TEST(VmoTest, FlagsGetReadOnly) {
  zx::vmo ro_vmo;

  // Get a read only vmo.
  {
    zx::vmo rw_vmo;
    ASSERT_NO_FAILURES(GetVmo(rw_vmo));
    zx_info_handle_basic_t info;
    ASSERT_OK(rw_vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
    ASSERT_OK(rw_vmo.duplicate(info.rights & ~ZX_RIGHT_WRITE, &ro_vmo));
  }

  zx::stream ro_stream;
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ, ro_vmo, kInitialSeek, &ro_stream));
  zxio_storage_t storage;
  ASSERT_OK(zxio::CreateVmo(&storage, std::move(ro_vmo), std::move(ro_stream)));
  zxio_t* io = &storage.io;

  uint64_t raw_flags{};
  ASSERT_STATUS(ZX_OK, zxio_flags_get(io, &raw_flags));
  fuchsia_io::wire::Flags flags{raw_flags};
  EXPECT_TRUE(flags & fuchsia_io::wire ::Flags::kPermReadBytes);
  EXPECT_FALSE(flags & fuchsia_io::wire ::Flags::kPermWriteBytes);
  EXPECT_FALSE(flags & fuchsia_io::wire::Flags::kPermExecute);

  zxio_destroy(io);
}

TEST(VmoTest, FlagsGetReadExec) {
  zx::vmo exec_vmo;

  // Get a read + exec vmo.
  {
    zx::vmo rw_vmo;
    ASSERT_NO_FAILURES(GetVmo(rw_vmo));
    zx_info_handle_basic_t info;
    ASSERT_OK(rw_vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
    zx::vmo ro_vmo;
    ASSERT_OK(rw_vmo.duplicate(info.rights & ~ZX_RIGHT_WRITE, &ro_vmo));
    zx::result vmex_getter = component::Connect<fuchsia_kernel::VmexResource>();
    ASSERT_OK(vmex_getter.status_value());
    fit::result vmex_result = fidl::Call(*vmex_getter)->Get();
    ASSERT_TRUE(vmex_result.is_ok(), "%s", vmex_result.error_value().FormatDescription().c_str());
    zx::resource vmex = std::move(vmex_result.value().resource());
    ASSERT_OK(ro_vmo.replace_as_executable(vmex, &exec_vmo));
  }

  zx::stream exec_stream;
  ASSERT_OK(zx::stream::create(ZX_STREAM_MODE_READ, exec_vmo, kInitialSeek, &exec_stream));
  zxio_storage_t storage_for_exec;
  ASSERT_OK(zxio::CreateVmo(&storage_for_exec, std::move(exec_vmo), std::move(exec_stream)));
  zxio_t* exec_io = &storage_for_exec.io;

  uint64_t raw_flags{};
  ASSERT_STATUS(ZX_OK, zxio_flags_get(exec_io, &raw_flags));
  fuchsia_io::wire::Flags flags{raw_flags};
  EXPECT_TRUE(flags & fuchsia_io::wire ::Flags::kPermReadBytes);
  EXPECT_FALSE(flags & fuchsia_io::wire ::Flags::kPermWriteBytes);
  EXPECT_TRUE(flags & fuchsia_io::wire::Flags::kPermExecute);

  zxio_destroy(exec_io);
}

}  // namespace
