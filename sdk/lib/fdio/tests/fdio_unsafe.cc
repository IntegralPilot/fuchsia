// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/namespace.h>
#include <lib/fdio/unsafe.h>
#include <unistd.h>

#include <fbl/unique_fd.h>
#include <zxtest/zxtest.h>

namespace {

TEST(UnsafeTest, BorrowChannel) {
  fbl::unique_fd fd(open("/pkg", O_DIRECTORY | O_RDONLY));
  ASSERT_LE(0, fd.get());

  fdio_t* io = fdio_unsafe_fd_to_io(fd.get());
  ASSERT_TRUE(io);

  auto dir = fidl::UnownedClientEnd<fuchsia_io::Node>(fdio_unsafe_borrow_channel(io));
  ASSERT_TRUE(dir.is_valid());

  auto [client_end, server_end] = fidl::Endpoints<fuchsia_unknown::Cloneable>::Create();
  auto result = fidl::WireCall(dir)->Clone(std::move(server_end));
  ASSERT_OK(result.status());

  fdio_unsafe_release(io);
  fd.reset();
}

TEST(UnsafeTest, BorrowChannelFromUnsupportedObject) {
  // Local namespaces do not have a backing channel, so |fdio_unsafe_borrow_channel| should fail.

  fdio_ns_t* ns;
  ASSERT_OK(fdio_ns_create(&ns));
  fbl::unique_fd fd(open("/pkg", O_RDONLY | O_DIRECTORY));
  ASSERT_LE(0, fd.get());
  ASSERT_OK(fdio_ns_bind_fd(ns, "/test-ns-item", fd.get()));
  ASSERT_EQ(0, close(fd.release()));

  fbl::unique_fd ns_fd(fdio_ns_opendir(ns));
  ASSERT_LE(0, ns_fd.get());
  fdio_t* io = fdio_unsafe_fd_to_io(ns_fd.get());
  ASSERT_TRUE(io);

  EXPECT_EQ(ZX_HANDLE_INVALID, fdio_unsafe_borrow_channel(io));
  fdio_unsafe_release(io);

  ASSERT_OK(fdio_ns_destroy(ns));
}

}  // namespace
