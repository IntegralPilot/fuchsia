// Copyright 2022 The Fuchsia Authors.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <lib/fdio/namespace.h>
#include <lib/fit/defer.h>
#include <lib/zxio/null.h>
#include <lib/zxio/ops.h>
#include <lib/zxio/zxio.h>
#include <sys/mman.h>
#include <sys/types.h>
#include <zircon/errors.h>
#include <zircon/processargs.h>

#include <cstddef>
#include <latch>
#include <sstream>
#include <thread>

#include <fbl/unique_fd.h>
#include <zxtest/zxtest.h>

namespace {

TEST(NamespaceTest, CreateDestroy) {
  fdio_ns_t* ns;
  ASSERT_OK(fdio_ns_create(&ns));
  ASSERT_OK(fdio_ns_destroy(ns));
}

// Test that namespace functions properly handle null path pointers.
TEST(NamespaceTest, NullPaths) {
  fdio_ns_t* ns;
  ASSERT_OK(fdio_ns_create(&ns));
  auto destroy = fit::defer([ns] { ASSERT_OK(fdio_ns_destroy(ns)); });

  zx::channel ch0, ch1;
  ASSERT_OK(zx::channel::create(0, &ch0, &ch1));

  EXPECT_STATUS(fdio_ns_bind(ns, nullptr, ch0.release()), ZX_ERR_INVALID_ARGS);

  EXPECT_STATUS(fdio_ns_unbind(ns, nullptr), ZX_ERR_INVALID_ARGS);

  EXPECT_FALSE(fdio_ns_is_bound(ns, nullptr));

  fbl::unique_fd fd(memfd_create("TestFd", 0));
  ASSERT_GE(fd.get(), 0);
  EXPECT_STATUS(fdio_ns_bind_fd(ns, nullptr, fd.get()), ZX_ERR_INVALID_ARGS);

  zx::channel service0, service1;
  ASSERT_OK(zx::channel::create(0, &service0, &service1));
  EXPECT_STATUS(fdio_ns_open3(ns, nullptr, 0, service0.release()), ZX_ERR_INVALID_ARGS);
}

TEST(NamespaceTest, BindUnbindRoot) {
  fdio_ns_t* ns;
  ASSERT_OK(fdio_ns_create(&ns));
  auto destroy = fit::defer([ns] { ASSERT_OK(fdio_ns_destroy(ns)); });

  zx::channel ch0, ch1;
  ASSERT_OK(zx::channel::create(0, &ch0, &ch1));

  ASSERT_OK(fdio_ns_bind(ns, "/", ch0.release()));

  ASSERT_OK(fdio_ns_unbind(ns, "/"));
}

TEST(NamespaceTest, BindUnbindCanonicalPaths) {
  fdio_ns_t* ns;
  ASSERT_OK(fdio_ns_create(&ns));
  auto destroy = fit::defer([ns] { ASSERT_OK(fdio_ns_destroy(ns)); });

  zx::channel ch0, ch1;
  ASSERT_OK(zx::channel::create(0, &ch0, &ch1));

  ASSERT_OK(fdio_ns_bind(ns, "/foo", ch0.release()));

  ASSERT_OK(fdio_ns_unbind(ns, "/foo"));
}

TEST(NamespaceTest, BindUnbindNonCanonical) {
  fdio_ns_t* ns;
  ASSERT_OK(fdio_ns_create(&ns));
  auto destroy = fit::defer([ns] { ASSERT_OK(fdio_ns_destroy(ns)); });

  zx::channel ch0, ch1;
  ASSERT_OK(zx::channel::create(0, &ch0, &ch1));

  // These non-canonical paths both canonicalize to "/foo".
  ASSERT_OK(fdio_ns_bind(ns, "/////foo", ch0.release()));

  ASSERT_OK(fdio_ns_unbind(ns, "/foo/fake_subdir/../"));
}

TEST(NamespaceTest, BindOversizedPath) {
  fdio_ns_t* ns;
  ASSERT_OK(fdio_ns_create(&ns));
  auto destroy = fit::defer([ns] { ASSERT_OK(fdio_ns_destroy(ns)); });

  zx::channel ch0, ch1;
  ASSERT_OK(zx::channel::create(0, &ch0, &ch1));

  std::string long_path = "/";
  long_path.append(PATH_MAX - 1, 'a');

  // The largest legal path is PATH_MAX - 1 characters as PATH_MAX include space
  // for a null terminator. This path is thus too long by one character.
  EXPECT_EQ(long_path.length(), PATH_MAX);

  EXPECT_STATUS(fdio_ns_bind(ns, long_path.c_str(), ch0.release()), ZX_ERR_BAD_PATH);
}

TEST(NamespaceTest, BindOversizedPathComponent) {
  fdio_ns_t* ns;
  ASSERT_OK(fdio_ns_create(&ns));
  auto destroy = fit::defer([ns] { ASSERT_OK(fdio_ns_destroy(ns)); });

  zx::channel ch0, ch1;
  ASSERT_OK(zx::channel::create(0, &ch0, &ch1));

  std::string long_path_component = "/";
  // Path components are limited to up to NAME_MAX characters. This path
  // component is thus too long by one character.
  long_path_component.append(NAME_MAX + 1, 'a');

  EXPECT_STATUS(fdio_ns_bind(ns, long_path_component.c_str(), ch0.release()), ZX_ERR_BAD_PATH);
}

TEST(NamespaceTest, ConnectRoot) {
  fdio_ns_t* ns;
  ASSERT_OK(fdio_ns_create(&ns));
  auto destroy = fit::defer([ns] { ASSERT_OK(fdio_ns_destroy(ns)); });

  zx::channel ch0, ch1;
  ASSERT_OK(zx::channel::create(0, &ch0, &ch1));

  ASSERT_OK(fdio_ns_bind(ns, "/", ch0.release()));
  ASSERT_STATUS(ZX_ERR_TIMED_OUT,
                ch1.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite_past(), nullptr));

  zx::channel service0, service1;
  ASSERT_OK(zx::channel::create(0, &service0, &service1));
  ASSERT_OK(fdio_ns_open3(ns, "/foo", 1u, service0.release()));

  // Expect an incoming connect on ch1
  ASSERT_OK(ch1.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite_past(), nullptr));
}

TEST(NamespaceTest, ConnectCanonicalPath) {
  fdio_ns_t* ns;
  ASSERT_OK(fdio_ns_create(&ns));
  auto destroy = fit::defer([ns] { ASSERT_OK(fdio_ns_destroy(ns)); });

  zx::channel ch0, ch1;
  ASSERT_OK(zx::channel::create(0, &ch0, &ch1));

  ASSERT_OK(fdio_ns_bind(ns, "/foo", ch0.release()));
  ASSERT_STATUS(ZX_ERR_TIMED_OUT,
                ch1.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite_past(), nullptr));

  zx::channel service0, service1;
  ASSERT_OK(zx::channel::create(0, &service0, &service1));
  ASSERT_OK(fdio_ns_open3(ns, "/foo/bar", 1u, service0.release()));

  // Expect an incoming connect on ch1
  ASSERT_OK(ch1.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite_past(), nullptr));
}

TEST(NamespaceTest, ConnectNonCanonicalPath) {
  fdio_ns_t* ns;
  ASSERT_OK(fdio_ns_create(&ns));
  auto destroy = fit::defer([ns] { ASSERT_OK(fdio_ns_destroy(ns)); });

  zx::channel ch0, ch1;
  ASSERT_OK(zx::channel::create(0, &ch0, &ch1));

  ASSERT_OK(fdio_ns_bind(ns, "/foo", ch0.release()));
  ASSERT_STATUS(ZX_ERR_TIMED_OUT,
                ch1.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite_past(), nullptr));

  zx::channel service0, service1;
  ASSERT_OK(zx::channel::create(0, &service0, &service1));
  ASSERT_OK(fdio_ns_open3(ns, "//foo/fake_subdir/.././Service", 1u, service0.release()));

  // Expect an incoming connect on ch1
  ASSERT_OK(ch1.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite_past(), nullptr));
}

TEST(NamespaceTest, ConnectOversizedPath) {
  fdio_ns_t* ns;
  ASSERT_OK(fdio_ns_create(&ns));
  auto destroy = fit::defer([ns] { ASSERT_OK(fdio_ns_destroy(ns)); });

  zx::channel ch0, ch1;
  ASSERT_OK(zx::channel::create(0, &ch0, &ch1));

  std::string long_path = "/";
  long_path.append(PATH_MAX - 1, 'a');

  // The largest legal path is PATH_MAX - 1 characters as PATH_MAX include space
  // for a null terminator. This path is thus too long by one character.
  EXPECT_EQ(long_path.length(), PATH_MAX);

  EXPECT_STATUS(fdio_ns_open3(ns, long_path.c_str(), 0u, ch0.release()), ZX_ERR_BAD_PATH);
}

TEST(NamespaceTest, ConnectOversizedPathComponent) {
  fdio_ns_t* ns;
  ASSERT_OK(fdio_ns_create(&ns));
  auto destroy = fit::defer([ns] { ASSERT_OK(fdio_ns_destroy(ns)); });

  zx::channel ch0, ch1;
  ASSERT_OK(zx::channel::create(0, &ch0, &ch1));

  std::string long_path_component = "/";
  // Path components are limited to up to NAME_MAX characters. This path
  // component is thus too long by one character.
  long_path_component.append(NAME_MAX + 1, 'a');

  EXPECT_STATUS(fdio_ns_open3(ns, long_path_component.c_str(), 0u, ch0.release()), ZX_ERR_BAD_PATH);
}

TEST(NamespaceTest, ExportBindUnbindRace) {
  fdio_ns_t* ns;
  ASSERT_OK(fdio_ns_create(&ns));
  auto destroy = fit::defer([ns] { ASSERT_OK(fdio_ns_destroy(ns)); });

  constexpr int8_t num_threads = 5;
  std::latch latch(static_cast<ptrdiff_t>(2) * num_threads);

  std::vector<std::thread> threads;
  for (int8_t i = 0; i < num_threads; ++i) {
    threads.emplace_back(
        [&](char c) {
          std::ostringstream path;
          path << "/" << c;
          const std::string path_str = path.str();

          zx::channel ch0, ch1;
          ASSERT_OK(zx::channel::create(0, &ch0, &ch1));

          latch.arrive_and_wait();

          ASSERT_OK(fdio_ns_bind(ns, path_str.c_str(), ch0.release()));
          ASSERT_OK(fdio_ns_unbind(ns, path_str.c_str()));
        },
        'A' + i);

    threads.emplace_back([&]() {
      fdio_flat_namespace_t* flat;

      latch.arrive_and_wait();

      ASSERT_OK(fdio_ns_export(ns, &flat));
      fdio_ns_free_flat_ns(flat);
    });
  }

  for (auto& thread : threads) {
    thread.join();
  }
}

TEST(NamespaceTest, BindShadowingFails) {
  constexpr std::tuple<const char*, const char*, zx_status_t> test_cases[] = {
      {"/", "/foo", ZX_ERR_NOT_SUPPORTED},
      {"/foo", "/", ZX_ERR_NOT_SUPPORTED},
      {"/foo", "/foo/bar", ZX_ERR_NOT_SUPPORTED},
      {"/foo/bar", "/foo", ZX_ERR_ALREADY_EXISTS},
  };
  for (const auto& [first, second, expected] : test_cases) {
    SCOPED_TRACE(std::string(first) + ", " + std::string(second));

    fdio_ns_t* ns;
    ASSERT_OK(fdio_ns_create(&ns));
    auto destroy = fit::defer([ns] { ASSERT_OK(fdio_ns_destroy(ns)); });

    zx::channel ch0, ch1;
    ASSERT_OK(zx::channel::create(0, &ch0, &ch1));

    ASSERT_OK(fdio_ns_bind(ns, first, ch0.release()));

    zx::channel ch2, ch3;
    ASSERT_OK(zx::channel::create(0, &ch2, &ch3));
    EXPECT_STATUS(fdio_ns_bind(ns, second, ch2.release()), expected);
  }
}

TEST(NamespaceTest, LocalBinding) {
  auto on_test_opened = [](zxio_storage_t* storage, void* context, zxio_ops_t const** ops) {
    static constexpr zxio_ops_t test_ops = []() {
      zxio_ops_t ops = zxio_default_ops;
      ops.attr_get = [](zxio_t* io, zxio_node_attributes_t* inout_attr) {
        if (inout_attr->has.abilities) {
          ZXIO_NODE_ATTR_SET(*inout_attr, abilities, ZXIO_OPERATION_GET_ATTRIBUTES);
        }
        return ZX_OK;
      };
      return ops;
    }();
    EXPECT_NE(storage, nullptr);
    EXPECT_NE(context, nullptr);
    *ops = &test_ops;
    return *static_cast<zx_status_t*>(context);
  };

  fdio_ns_t* root;
  ASSERT_OK(fdio_ns_get_installed(&root));

  ASSERT_EQ(fdio_ns_bind_local(root, "/local/dir/", on_test_opened, nullptr), ZX_ERR_INVALID_ARGS);

  const char* local_file = "/local/file";

  ASSERT_EQ(access(local_file, F_OK), -1);
  ASSERT_EQ(errno, ENOENT, "%s", strerror(errno));

  zx_status_t context = ZX_ERR_NOT_SUPPORTED;
  ASSERT_OK(fdio_ns_bind_local(root, local_file, on_test_opened, &context));

  // Test when callback fails
  ASSERT_EQ(access(local_file, F_OK), -1);
  ASSERT_EQ(errno, EOPNOTSUPP, "%s", strerror(errno));

  // Test when callback succeeds
  context = ZX_OK;
  ASSERT_EQ(access(local_file, F_OK), 0);

  DIR* dir = opendir("/local/");
  ASSERT_NE(dir, nullptr);
  bool found_file = false;
  while (struct dirent* entry = readdir(dir)) {
    if (std::string(entry->d_name) == "file") {
      found_file = true;
      break;
    }
  }
  closedir(dir);
  ASSERT_TRUE(found_file);

  ASSERT_OK(fdio_ns_unbind(root, local_file));
  ASSERT_EQ(access(local_file, F_OK), -1);
  ASSERT_EQ(errno, ENOENT, "%s", strerror(errno));
}

}  // namespace

#pragma clang diagnostic push
