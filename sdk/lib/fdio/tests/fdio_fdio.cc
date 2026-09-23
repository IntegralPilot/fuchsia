// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/fdio/fdio.h>
#include <lib/fdio/limits.h>
#include <lib/fdio/unsafe.h>
#include <lib/zx/channel.h>
#include <lib/zx/socket.h>
#include <lib/zx/vmo.h>
#include <lib/zxio/zxio.h>
#include <unistd.h>
#include <zircon/limits.h>

#include <fbl/unique_fd.h>
#include <zxtest/zxtest.h>

#include "predicates.h"

namespace {

TEST(FDIOTest, CreateNull) {
  fdio_t* io = fdio_null_create();
  fbl::unique_fd fd(fdio_bind_to_fd(io, -1, 0));
  EXPECT_LE(0, fd.get());
  EXPECT_EQ(3, write(fd.get(), "abc", 3));
}

TEST(FDIOTest, CreateNullFD) {
  fbl::unique_fd fd(fdio_fd_create_null());
  EXPECT_LE(0, fd.get());
  EXPECT_EQ(3, write(fd.get(), "abc", 3));
}

TEST(FDIOTest, DupNullFD) {
  fbl::unique_fd fd(fdio_fd_create_null());
  EXPECT_LE(0, fd.get());
  fbl::unique_fd dup_fd(dup(fd.get()));
  EXPECT_LE(0, dup_fd.get());
  EXPECT_NE(fd.get(), dup_fd.get());
}

TEST(FDIOTest, CreateSocket) {
  zx::socket s1, s2;
  ASSERT_OK(zx::socket::create(0, &s1, &s2));

  fdio_t* io = nullptr;
  ASSERT_OK(fdio_create(s1.release(), &io));
  ASSERT_TRUE(io);

  fbl::unique_fd fd(fdio_bind_to_fd(io, -1, 0));
  EXPECT_LE(0, fd.get());
  constexpr char payload[] = "abc";
  EXPECT_EQ(write(fd.get(), payload, sizeof(payload)), ssize_t(sizeof(payload)));

  char buffer[sizeof(payload) + 1] = {};
  size_t actual;
  EXPECT_OK(s2.read(0, buffer, sizeof(buffer), &actual));
  EXPECT_EQ(actual, sizeof(payload));
  EXPECT_STREQ(buffer, payload);
}

TEST(FDIOTest, CreateVMO) {
  const size_t kSize = zx_system_get_page_size();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kSize, 0, &vmo));

  fdio_t* io = nullptr;
  ASSERT_OK(fdio_create(vmo.release(), &io));
  ASSERT_TRUE(io);

  fbl::unique_fd fd(fdio_bind_to_fd(io, -1, 0));
  EXPECT_LE(0, fd.get());
  constexpr char payload[] = "xyz";
  EXPECT_EQ(write(fd.get(), payload, sizeof(payload)), ssize_t(sizeof(payload)));

  char buffer[sizeof(payload) + 1] = {};
  ssize_t actual = pread(fd.get(), buffer, sizeof(buffer), 0);
  EXPECT_EQ(actual, ssize_t(sizeof(buffer)));
  EXPECT_STREQ(buffer, payload);
}

TEST(FDIOTest, BindToFDAgain) {
  zx::socket s1, s2;
  ASSERT_OK(zx::socket::create(0, &s1, &s2));

  fdio_t* io = nullptr;
  ASSERT_OK(fdio_create(s1.release(), &io));

  fbl::unique_fd fd(fdio_bind_to_fd(io, -1, 0));
  EXPECT_LE(0, fd.get());
  EXPECT_EQ(0, fcntl(fd.get(), F_GETFD));

  fdio_t* io2 = fdio_null_create();
  fbl::unique_fd fd2(fdio_bind_to_fd(io2, fd.get(), -1));
  EXPECT_EQ(fd.get(), fd2.get());
  (void)fd.release();

  zx_signals_t observed = 0;
  ASSERT_OK(s2.wait_one(ZX_SOCKET_PEER_CLOSED, zx::time::infinite_past(), &observed));
  ASSERT_TRUE(observed & ZX_SOCKET_PEER_CLOSED);
}

bool find_unused_fd(int starting_fd, int* out_fd) {
  for (int fd = starting_fd; fd < FDIO_MAX_FD; ++fd) {
    if (fcntl(fd, F_GETFD) == -1) {
      *out_fd = fd;
      return true;
    }
  }
  return false;
}

TEST(FDIOTest, UnbindFromFD) {
  int unused_fd = 0;
  ASSERT_TRUE(find_unused_fd(37, &unused_fd));
  ASSERT_EQ(-1, fcntl(unused_fd, F_GETFD));

  fdio_t* io = fdio_null_create();
  fbl::unique_fd fd(fdio_bind_to_fd(io, unused_fd, -1));
  EXPECT_EQ(unused_fd, fd.get());
  EXPECT_EQ(0, fcntl(unused_fd, F_GETFD));
  fdio_t* io2 = nullptr;
  EXPECT_OK(fdio_unbind_from_fd(fd.get(), &io2));
  EXPECT_EQ(-1, fcntl(unused_fd, F_GETFD));
  (void)fd.release();
  EXPECT_EQ(io, io2);
  fdio_unsafe_release(io2);

  // Invalid file descriptors.
  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, fdio_unbind_from_fd(-1, &io2));
  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, fdio_unbind_from_fd(FDIO_MAX_FD, &io2));
}

TEST(FDIOTest, GetZxio) {
  zxio_storage_t* storage = nullptr;
  fdio_t* fdio = fdio_zxio_create(&storage);
  ASSERT_NE(nullptr, fdio);
  zxio_t* zxio = fdio_get_zxio(fdio);
  EXPECT_EQ(storage, reinterpret_cast<zxio_storage_t*>(zxio));
  fdio_unsafe_release(fdio);
}

}  // namespace
