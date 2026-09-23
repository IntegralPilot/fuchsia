// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/svc/dir.h"

#include <fcntl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async-loop/testing/cpp/real_loop.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/vfs/cpp/pseudo_file.h>
#include <lib/zx/channel.h>
#include <zircon/errors.h>

#include <cstdint>
#include <cstring>
#include <memory>
#include <thread>

#include <fbl/unique_fd.h>
#include <zxtest/zxtest.h>

namespace fio = fuchsia_io;

namespace svc {
namespace {

void connect(void* context, const char* service_name, zx_handle_t service_request) {
  EXPECT_EQ(std::string("foobar"), service_name);
  zx::channel binding(service_request);
  zx_signals_t observed;
  EXPECT_OK(binding.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), &observed));
  EXPECT_STATUS(binding.read(ZX_CHANNEL_READ_MAY_DISCARD, nullptr, nullptr, 0, 0, nullptr, nullptr),
                ZX_ERR_BUFFER_TOO_SMALL);
  EXPECT_OK(binding.write(0, "ok", 2, nullptr, 0));
}

// Convenience wrappers for testing only.
zx_status_t svc_directory_add_service_unsized(svc_dir_t* dir, std::string_view path,
                                              std::string_view name, void* context,
                                              svc_connector_t* handler) {
  return svc_directory_add_service(dir, path.data(), path.length(), name.data(), name.length(),
                                   context, handler);
}

zx_status_t svc_directory_add_directory_unsized(svc_dir_t* dir, std::string_view path,
                                                std::string_view name, zx_handle_t subdir) {
  return svc_directory_add_directory(dir, path.data(), path.size(), name.data(), name.size(),
                                     subdir);
}

zx_status_t svc_directory_remove_entry_unsized(svc_dir_t* dir, std::string_view path,
                                               std::string_view name) {
  return svc_directory_remove_entry(dir, path.data(), path.size(), name.data(), name.size());
}

class ServiceTest : public loop_fixture::RealLoop, public zxtest::Test {
 protected:
  // Destroys `dir` and waits until all connections have been closed.
  zx_status_t DestroyAndWait(svc_dir_t* dir) {
    zx_status_t status = ::svc_directory_destroy(dir);
    // Cleanup is asynchronous.
    RunLoopUntilIdle();
    return status;
  }
};

TEST_F(ServiceTest, Control) {
  zx::channel dir, dir_request;
  ASSERT_OK(zx::channel::create(0, &dir, &dir_request));

  std::thread child([this, dir_request = std::move(dir_request)]() mutable {
    svc_dir_t* dir = nullptr;
    EXPECT_OK(svc_directory_create(&dir));
    EXPECT_OK(svc_directory_serve(dir, dispatcher(), dir_request.release()));
    EXPECT_OK(svc_directory_add_service_unsized(dir, "svc", "foobar", nullptr, connect));
    EXPECT_OK(svc_directory_add_service_unsized(dir, "svc", "baz", nullptr, nullptr));
    EXPECT_STATUS(svc_directory_add_service_unsized(dir, "svc", "baz", nullptr, nullptr),
                  ZX_ERR_ALREADY_EXISTS);
    EXPECT_OK(svc_directory_remove_entry_unsized(dir, "svc", "baz"));
    EXPECT_OK(svc_directory_add_service_unsized(dir, "another", "qux", nullptr, nullptr));

    RunLoop();

    ASSERT_OK(DestroyAndWait(dir));
  });

  // Verify that we can connect to a foobar service and get a response.
  zx::channel svc, request;
  ASSERT_OK(zx::channel::create(0, &svc, &request));
  ASSERT_OK(fdio_service_connect_at(dir.get(), "svc/foobar", request.release()));
  EXPECT_OK(svc.write(0, "hello", 5, nullptr, 0));
  zx_signals_t observed;
  EXPECT_OK(svc.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), &observed));
  EXPECT_STATUS(svc.read(ZX_CHANNEL_READ_MAY_DISCARD, nullptr, nullptr, 0, 0, nullptr, nullptr),
                ZX_ERR_BUFFER_TOO_SMALL);

  // Verify that connection to a removed service fails.
  ASSERT_OK(zx::channel::create(0, &svc, &request));
  ASSERT_OK(fdio_service_connect_at(dir.get(), "svc/baz", request.release()));
  EXPECT_OK(svc.wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), &observed));

  // Shutdown the service thread.
  QuitLoop();
  child.join();

  // Verify that connection fails after DestroyAndWait().
  ASSERT_OK(zx::channel::create(0, &svc, &request));
  ASSERT_OK(fdio_service_connect_at(dir.get(), "svc/foobar", request.release()));
  EXPECT_OK(svc.wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), &observed));
}

TEST_F(ServiceTest, PublishLegacyService) {
  zx::channel dir, dir_request;
  ASSERT_OK(zx::channel::create(0, &dir, &dir_request));

  std::thread child([this, dir_request = std::move(dir_request)]() mutable {
    svc_dir_t* dir = nullptr;
    EXPECT_OK(svc_directory_create(&dir));
    EXPECT_OK(svc_directory_serve(dir, dispatcher(), dir_request.release()));
    EXPECT_OK(svc_directory_add_service_unsized(dir, "", "foobar", nullptr, connect));
    EXPECT_OK(svc_directory_add_service_unsized(dir, "", "baz", nullptr, connect));
    EXPECT_OK(svc_directory_remove_entry_unsized(dir, "", "baz"));

    RunLoop();

    ASSERT_OK(DestroyAndWait(dir));
  });

  // Verify that we can connect to a foobar service and get a response.
  zx::channel svc, request;
  ASSERT_OK(zx::channel::create(0, &svc, &request));
  ASSERT_OK(fdio_service_connect_at(dir.get(), "foobar", request.release()));
  EXPECT_OK(svc.write(0, "hello", 5, nullptr, 0));
  zx_signals_t observed;
  EXPECT_OK(svc.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), &observed));
  EXPECT_STATUS(svc.read(ZX_CHANNEL_READ_MAY_DISCARD, nullptr, nullptr, 0, 0, nullptr, nullptr),
                ZX_ERR_BUFFER_TOO_SMALL);

  // Verify that connection to a removed service fails.
  ASSERT_OK(zx::channel::create(0, &svc, &request));
  ASSERT_OK(fdio_service_connect_at(dir.get(), "baz", request.release()));
  EXPECT_OK(svc.wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), &observed));

  // Shutdown the service thread.
  QuitLoop();
  child.join();

  // Verify that connection fails after DestroyAndWait().
  ASSERT_OK(zx::channel::create(0, &svc, &request));
  ASSERT_OK(fdio_service_connect_at(dir.get(), "foobar", request.release()));
  EXPECT_OK(svc.wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), &observed));
}

TEST_F(ServiceTest, ConnectsByPath) {
  zx::channel dir, dir_request;
  ASSERT_OK(zx::channel::create(0, &dir, &dir_request));

  std::thread child([this, dir_request = std::move(dir_request)]() mutable {
    svc_dir_t* dir = nullptr;
    EXPECT_OK(svc_directory_create(&dir));
    EXPECT_OK(svc_directory_serve(dir, dispatcher(), dir_request.release()));
    ASSERT_OK(svc_directory_add_service_unsized(dir, "svc/fuchsia.logger.LogSink/default", "foobar",
                                                nullptr, connect));

    RunLoop();

    ASSERT_OK(DestroyAndWait(dir));
  });

  // Verify that we can connect to svc/fuchsia.logger.LogSink/default/foobar
  // and get a response.
  zx::channel svc, request;
  ASSERT_OK(zx::channel::create(0, &svc, &request));
  ASSERT_OK(fdio_service_connect_at(dir.get(), "svc/fuchsia.logger.LogSink/default/foobar",
                                    request.release()));
  EXPECT_OK(svc.write(0, "hello", 5, nullptr, 0));
  zx_signals_t observed;
  EXPECT_OK(svc.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), &observed));
  EXPECT_STATUS(svc.read(ZX_CHANNEL_READ_MAY_DISCARD, nullptr, nullptr, 0, 0, nullptr, nullptr),
                ZX_ERR_BUFFER_TOO_SMALL);

  // Shutdown the service thread.
  QuitLoop();
  child.join();
}

TEST_F(ServiceTest, RejectsMalformedPaths) {
  zx::channel _directory, dir_request;
  ASSERT_OK(zx::channel::create(0, &_directory, &dir_request));

  svc_dir_t* dir = nullptr;
  EXPECT_OK(svc_directory_create(&dir));
  EXPECT_OK(svc_directory_serve(dir, dispatcher(), dir_request.release()));

  // The following paths should all fail.
  EXPECT_STATUS(svc_directory_add_service_unsized(dir, "/", "foobar", nullptr, connect),
                ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(svc_directory_add_service_unsized(dir, "/svc", "foobar", nullptr, connect),
                ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(svc_directory_add_service_unsized(dir, "/svc//foo", "foobar", nullptr, connect),
                ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(svc_directory_add_service_unsized(dir, "svc/", "foobar", nullptr, connect),
                ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(svc_directory_add_service_unsized(dir, ".", "foobar", nullptr, connect),
                ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(svc_directory_add_service_unsized(dir, "..", "foobar", nullptr, connect),
                ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(svc_directory_add_service_unsized(dir, "...", "foobar", nullptr, connect),
                ZX_ERR_INVALID_ARGS);
  EXPECT_STATUS(svc_directory_add_service_unsized(dir, "svc/..", "foobar", nullptr, connect),
                ZX_ERR_INVALID_ARGS);

  // Cleanup resources.
  ASSERT_OK(DestroyAndWait(dir));
}

TEST_F(ServiceTest, AddSubdDirByPath) {
  static constexpr char kTestPath[] = "root/of/directory";
  static constexpr char kTestDirectory[] = "foobar";
  static constexpr char kTestFile[] = "sample.txt";
  static constexpr char kTestContent[] = "Hello World!";
  static constexpr size_t kMaxFileSize = 1024;

  zx::channel dir, dir_request;
  ASSERT_OK(zx::channel::create(0, &dir, &dir_request));

  std::thread child([this, dir_request = std::move(dir_request)]() mutable {
    svc_dir_t* dir = nullptr;
    ASSERT_OK(svc_directory_create(&dir));
    ASSERT_OK(svc_directory_serve(dir, dispatcher(), dir_request.release()));

    auto subdir = std::make_unique<vfs::PseudoDir>();
    ASSERT_OK(subdir->AddEntry(
        kTestFile,
        std::make_unique<vfs::PseudoFile>(
            kMaxFileSize,
            /*read_handler=*/[](std::vector<uint8_t>* output, size_t max_bytes) -> zx_status_t {
              for (const char& c : kTestContent) {
                output->push_back(c);
              }
              return ZX_OK;
            })));
    auto [client_end, server_end] = fidl::Endpoints<fio::Directory>::Create();
    constexpr auto kServeFlags = static_cast<fio::Flags>(static_cast<uint64_t>(fio::kRwStarDir));
    ASSERT_OK(subdir->Serve(kServeFlags, std::move(server_end), dispatcher()));

    ASSERT_OK(svc_directory_add_directory_unsized(dir, kTestPath, kTestDirectory,
                                                  client_end.TakeChannel().release()));

    RunLoop();

    EXPECT_OK(svc_directory_remove_entry_unsized(dir, kTestPath, kTestDirectory));
    ASSERT_OK(DestroyAndWait(dir));
    // **NOTE**: Similar to `dir`, we must explicitly destroy `subdir` and wait for any active
    // connections to be closed. These types are not thread safe (they must be used with a single
    // threaded dispatcher), and cannot rely on channel-initiated teardown from the main thread.
    subdir = nullptr;
    RunLoopUntilIdle();
  });

  fbl::unique_fd root_fd;
  ASSERT_OK(fdio_fd_create(dir.release(), root_fd.reset_and_get_address()));

  std::string directory_path = std::string(kTestPath) + "/" + std::string(kTestDirectory);
  fbl::unique_fd dir_fd(openat(root_fd.get(), directory_path.c_str(), O_DIRECTORY));
  ASSERT_TRUE(dir_fd.is_valid(), "Failed to open directory \"%s\": %s", directory_path.c_str(),
              strerror(errno));

  fbl::unique_fd file_fd(openat(dir_fd.get(), kTestFile, O_RDONLY));
  ASSERT_TRUE(file_fd.is_valid(), "Failed to open file \"%s\": %s", kTestFile, strerror(errno));
  static constexpr size_t kMaxBufferSize = 1024;
  static char kReadBuffer[kMaxBufferSize];
  ssize_t bytes_read = read(file_fd.get(), reinterpret_cast<void*>(kReadBuffer), kMaxBufferSize);
  ASSERT_GE(bytes_read, 0, "Read 0 bytes from file at \"%s\": %s", kTestFile, strerror(errno));

  std::string actual_content(kReadBuffer, bytes_read - 1 /* Minus NULL terminator */);
  EXPECT_EQ(actual_content, kTestContent);

  QuitLoop();
  child.join();
}

TEST_F(ServiceTest, AddDirFailsOnBadInput) {
  // |dir| is nullptr
  {
    zx::channel _server_end, client_end;
    ASSERT_OK(zx::channel::create(0, &_server_end, &client_end));
    EXPECT_STATUS(svc_directory_add_directory_unsized(/*dir=*/nullptr, "", "AValidEntry",
                                                      client_end.release()),
                  ZX_ERR_INVALID_ARGS);
  }

  // |name| is nullptr
  {
    zx::channel _directory, dir_request;
    ASSERT_OK(zx::channel::create(0, &_directory, &dir_request));

    svc_dir_t* dir = nullptr;
    ASSERT_OK(svc_directory_create(&dir));
    ASSERT_OK(svc_directory_serve(dir, dispatcher(), dir_request.release()));

    zx::channel _subdir, subdir_client;
    ASSERT_OK(zx::channel::create(0, &_subdir, &subdir_client));

    EXPECT_STATUS(svc_directory_add_directory(dir, /*path=*/nullptr, 0, /*name=*/nullptr, 0,
                                              subdir_client.release()),
                  ZX_ERR_INVALID_ARGS);

    ASSERT_OK(DestroyAndWait(dir));
  }

  // |subdir| is an invalid handle
  {
    zx::channel _directory, dir_request;
    ASSERT_OK(zx::channel::create(0, &_directory, &dir_request));

    svc_dir_t* dir = nullptr;
    ASSERT_OK(svc_directory_create(&dir));
    ASSERT_OK(svc_directory_serve(dir, dispatcher(), dir_request.release()));

    EXPECT_STATUS(svc_directory_add_directory_unsized(dir, /*path=*/"", "AValidEntry",
                                                      /*subdir=*/ZX_HANDLE_INVALID),
                  ZX_ERR_INVALID_ARGS);

    ASSERT_OK(DestroyAndWait(dir));
  }

  // |path| is invalid
  {
    static constexpr const char* kBadPaths[] = {"//", "/foo/", "foo//bar", "foo/"};

    zx::channel _directory, dir_request;
    ASSERT_OK(zx::channel::create(0, &_directory, &dir_request));

    svc_dir_t* dir = nullptr;
    ASSERT_OK(svc_directory_create(&dir));
    ASSERT_OK(svc_directory_serve(dir, dispatcher(), dir_request.release()));

    zx::channel _subdir, subdir_client;
    ASSERT_OK(zx::channel::create(0, &_subdir, &subdir_client));

    for (auto bad_path : kBadPaths) {
      EXPECT_STATUS(svc_directory_add_directory_unsized(
                        dir, /*path=*/bad_path, /*name=*/"AValidEntry", subdir_client.release()),
                    ZX_ERR_INVALID_ARGS);
    }

    ASSERT_OK(DestroyAndWait(dir));
  }
}

TEST_F(ServiceTest, Rights) {
  zx::channel dir, dir_request;
  ASSERT_OK(zx::channel::create(0, &dir, &dir_request));

  std::thread child([this, dir_request = std::move(dir_request)]() mutable {
    svc_dir_t* dir = nullptr;
    EXPECT_OK(svc_directory_create(&dir));
    EXPECT_OK(svc_directory_serve(dir, dispatcher(), dir_request.release()));

    RunLoop();

    ASSERT_OK(DestroyAndWait(dir));
  });

  // Turn dir into an file descriptor so open calls synchronously ensure the open completed
  // successfully.
  fbl::unique_fd root_fd;
  ASSERT_OK(fdio_fd_create(dir.release(), root_fd.reset_and_get_address()));

  // Verify that we can open the directory with rx permissions.
  fbl::unique_fd new_fd;
  ASSERT_OK(fdio_open3_fd_at(root_fd.get(), ".", static_cast<uint64_t>(fio::kRxStarDir),
                             new_fd.reset_and_get_address()));

  // Shutdown the service thread.
  QuitLoop();
  child.join();
}

}  // namespace
}  // namespace svc
