// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fit/defer.h>
#include <lib/zxio/ops.h>
#include <lib/zxio/types.h>
#include <string.h>

#include <algorithm>
#include <atomic>
#include <limits>
#include <memory>

#include <zxtest/zxtest.h>

#include "sdk/lib/zxio/private.h"
#include "sdk/lib/zxio/tests/test_directory_server_base.h"

namespace {

namespace fio = fuchsia_io;

class TestServer final : public zxio_tests::TestDirectoryServerBase {
 public:
  constexpr static size_t kEntryCount = 1000;

  // Exercised by |zxio_close|.
  void Close(CloseCompleter::Sync& completer) final {
    num_close_.fetch_add(1);
    completer.ReplySuccess();
  }

  void ReadDirents(ReadDirentsRequestView request, ReadDirentsCompleter::Sync& completer) override {
    size_t actual = 0;

    for (; index_ < kEntryCount; ++index_) {
      char name[ZXIO_MAX_FILENAME + 1];
      const int name_length = snprintf(name, sizeof(name), "%zu", index_);

      struct dirent {
        uint64_t inode;
        uint8_t size;
        uint8_t type;
        char name[0];
      } __PACKED;

      auto& entry = *reinterpret_cast<dirent*>(buffer_ + actual);
      const size_t entry_size = sizeof(dirent) + name_length;
      if (actual + entry_size > request->max_bytes) {
        break;
      }

      ASSERT_GE(name_length, 0);
      // No null termination
      memcpy(entry.name, name, name_length);

      if (name_length > std::numeric_limits<uint8_t>::max()) {
        return completer.Close(ZX_ERR_BAD_STATE);
      }
      entry.size = static_cast<uint8_t>(name_length);
      entry.inode = index_;

      actual += entry_size;
    }
    completer.Reply(ZX_OK, fidl::VectorView<uint8_t>::FromExternal(
                               reinterpret_cast<uint8_t*>(buffer_), actual));
  }

  void Rewind(RewindCompleter::Sync& completer) final {
    memset(buffer_, 0, sizeof(buffer_));
    index_ = 0;
    completer.Reply(ZX_OK);
  }

  uint32_t num_close() const { return num_close_.load(); }

 private:
  std::atomic<uint32_t> num_close_ = 0;
  char buffer_[fio::wire::kMaxBuf] = {};
  size_t index_ = 0;
};

class DirentTest : public zxtest::Test {
 protected:
  void SetUp() final {
    auto [client_end, server_end] = fidl::Endpoints<fio::Directory>::Create();
    ASSERT_OK(zxio_dir_init(&dir_, std::move(client_end)));
    server_ = std::make_unique<TestServer>();
    loop_ = std::make_unique<async::Loop>(&kAsyncLoopConfigNoAttachToCurrentThread);
    ASSERT_OK(loop_->StartThread("fake-filesystem"));
    fidl::BindServer(loop_->dispatcher(), std::move(server_end), server_.get());
  }

  void TearDown() final {
    ASSERT_EQ(0, server_->num_close());
    ASSERT_OK(zxio_close(&dir_.io));
    zxio_destroy(&dir_.io);
    ASSERT_EQ(1, server_->num_close());
  }

  auto InitIteratorDeferCleanup(zxio_dirent_iterator_t& iterator) {
    return zx::make_result(zxio_dirent_iterator_init(&iterator, &dir_.io),
                           fit::defer([&]() { zxio_dirent_iterator_destroy(&iterator); }));
  }

 private:
  zxio_storage_t dir_;
  std::unique_ptr<TestServer> server_;
  std::unique_ptr<async::Loop> loop_;
};

TEST_F(DirentTest, StandardBufferSize) {
  zxio_dirent_iterator_t iterator;
  zx::result cleanup = InitIteratorDeferCleanup(iterator);
  ASSERT_OK(cleanup);

  char name_buffer[ZXIO_MAX_FILENAME + 1];
  zxio_dirent_t entry = {.name = name_buffer};

  for (size_t count = 0; count < TestServer::kEntryCount; ++count) {
    ASSERT_OK(zxio_dirent_iterator_next(&iterator, &entry));
    EXPECT_TRUE(entry.has.id);
    EXPECT_EQ(entry.id, count);
    char name[ZXIO_MAX_FILENAME + 1];
    const int name_length = snprintf(name, sizeof(name), "%zu", count);
    EXPECT_EQ(entry.name_length, name_length);
    EXPECT_STREQ(std::string_view(name, name_length),
                 std::string_view(entry.name, entry.name_length));
  }
}

TEST_F(DirentTest, Rewind) {
  zxio_dirent_iterator_t iterator;
  zx::result cleanup = InitIteratorDeferCleanup(iterator);
  ASSERT_OK(cleanup);

  char name_buffer[ZXIO_MAX_FILENAME + 1];
  zxio_dirent_t entry = {.name = name_buffer};
  std::vector<std::string> names;
  names.reserve(TestServer::kEntryCount);

  for (size_t count = 0; count < TestServer::kEntryCount; ++count) {
    ASSERT_OK(zxio_dirent_iterator_next(&iterator, &entry));
    ASSERT_LE(entry.name_length, ZXIO_MAX_FILENAME);

    names.emplace_back(entry.name, entry.name_length);
  }

  ASSERT_OK(zxio_dirent_iterator_rewind(&iterator));

  for (size_t count = 0; count < TestServer::kEntryCount; ++count) {
    ASSERT_OK(zxio_dirent_iterator_next(&iterator, &entry));
    ASSERT_LE(entry.name_length, ZXIO_MAX_FILENAME);
    EXPECT_STREQ(std::string_view(entry.name, entry.name_length), names[count]);
  }
}

TEST_F(DirentTest, RewindHalfwayThrough) {
  zxio_dirent_iterator_t iterator;
  zx::result cleanup = InitIteratorDeferCleanup(iterator);
  ASSERT_OK(cleanup);

  constexpr size_t kHalfEntryCount = TestServer::kEntryCount / 2;
  static_assert(kHalfEntryCount != 0);

  char name_buffer[ZXIO_MAX_FILENAME + 1];
  zxio_dirent_t entry = {.name = name_buffer};
  std::vector<std::string> names;
  names.reserve(kHalfEntryCount);

  for (size_t count = 0; count < kHalfEntryCount; ++count) {
    ASSERT_OK(zxio_dirent_iterator_next(&iterator, &entry));
    ASSERT_LE(entry.name_length, ZXIO_MAX_FILENAME);

    names.emplace_back(entry.name, entry.name_length);
  }

  ASSERT_OK(zxio_dirent_iterator_rewind(&iterator));

  for (size_t count = 0; count < kHalfEntryCount; ++count) {
    ASSERT_OK(zxio_dirent_iterator_next(&iterator, &entry));
    ASSERT_LE(entry.name_length, ZXIO_MAX_FILENAME);
    EXPECT_STREQ(std::string_view(entry.name, entry.name_length), names[count]);
  }
}

}  // namespace
