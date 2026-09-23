// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// These tests verify basic functionality of `vfs::LazyDir`. For more comprehensive tests, see
// //src/storage/lib/vfs/cpp and //src/storage/conformance.

#include <fcntl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/io.h>
#include <lib/vfs/cpp/lazy_dir.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <sys/stat.h>
#include <zircon/status.h>

#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <string_view>

#include <fbl/unique_fd.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

namespace {

// LazyDir that acts like a pseudo-dir for ease of testing.
class TestLazyDir : public vfs::LazyDir {
 public:
  struct Entry {
    uint64_t id;
    uint32_t mode_type;
    std::unique_ptr<vfs::Node> node;
  };

  using EntryMap = std::map<std::string_view, Entry, std::less<>>;

  // Allows modification of the test directory's contents in a thread-safe manner.
  void SetEntries(EntryMap entries) {
    std::lock_guard guard(mutex_);
    entries_ = std::move(entries);
  }

 protected:
  void GetContents(std::vector<LazyEntry>* out_vector) const override {
    std::lock_guard guard(mutex_);
    *out_vector = std::vector<LazyEntry>();
    out_vector->reserve(entries_.size());
    for (const auto& [name, entry] : entries_) {
      out_vector->push_back(LazyEntry{
          .id = entry.id,
          .name = std::string(name),
          .type = entry.mode_type,
      });
    }
  }

  zx_status_t GetFile(Node** out_node, uint64_t id, std::string name) const override {
    std::lock_guard guard(mutex_);
    if (const auto entry = entries_.find(name); entry != entries_.cend()) {
      if (id != entry->second.id) {
        // The LazyDir type should guarantee consistency between the entry names and IDs.
        return ZX_ERR_BAD_STATE;
      }
      *out_node = entry->second.node.get();
      return ZX_OK;
    }
    return ZX_ERR_NOT_FOUND;
  }

 private:
  mutable std::mutex mutex_;
  EntryMap entries_ __TA_GUARDED(mutex_);
};

class LazyDirTest : public ::gtest::RealLoopFixture {
 protected:
  void SetUp() override {
    root_dir_ = std::make_unique<vfs::PseudoDir>();
    test_dir_ = std::make_shared<TestLazyDir>();
    root_dir_->AddSharedEntry("test_lazy_dir", test_dir_);
    auto [root_client, root_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(root_dir_->Serve(fuchsia_io::kPermReadable | fuchsia_io::kPermWritable,
                               std::move(root_server)),
              ZX_OK);
    root_client_ = std::move(root_client);
  }

  TestLazyDir& test_dir() { return *test_dir_; }

  // Consumes and opens the test directory as a file descriptor. This must be called from a
  // different thread than the one serving the connection and can only be called once.
  fbl::unique_fd open_test_dir_fd() {
    ZX_ASSERT_MSG(root_client_.is_valid(), "open_test_dir_fd() can only be called once per test!");
    fbl::unique_fd root_fd;
    zx_status_t status =
        fdio_fd_create(root_client_.TakeChannel().release(), root_fd.reset_and_get_address());
    ZX_ASSERT_MSG(status == ZX_OK, "Failed to create fd: %s", zx_status_get_string(status));

    fbl::unique_fd fd(openat(root_fd.get(), "test_lazy_dir", O_DIRECTORY));
    ZX_ASSERT_MSG(fd, "Failed to open entry: %s", strerror(errno));
    return fd;
  }

 private:
  std::unique_ptr<vfs::PseudoDir> root_dir_;
  std::shared_ptr<TestLazyDir> test_dir_;
  fidl::ClientEnd<fuchsia_io::Directory> root_client_;
};

TEST_F(LazyDirTest, ValidateEntries) {
  PerformBlockingWork([this] {
    auto root = open_test_dir_fd();

    // Directory should be empty initially.
    fbl::unique_fd fd(openat(root.get(), "does not exist", O_DIRECTORY));
    ASSERT_FALSE(fd) << "Opened non-existing entry!";
    ASSERT_EQ(errno, ENOENT) << strerror(errno);

    // Create some entries in the format:
    //    entry_a/
    //    entry_b/
    //        subentry
    TestLazyDir::EntryMap entries;

    auto entry_a = std::make_unique<vfs::PseudoDir>();
    entries["entry_a"] = TestLazyDir::Entry{
        .id = 2u,
        .mode_type = S_IFDIR,
        .node = std::move(entry_a),
    };

    auto entry_b = std::make_unique<vfs::PseudoDir>();
    auto subentry = std::make_unique<vfs::PseudoDir>();
    ASSERT_EQ(entry_b->AddEntry("subentry", std::move(subentry)), ZX_OK);
    entries["entry_b"] = TestLazyDir::Entry{
        .id = 5u,
        .mode_type = S_IFDIR,
        .node = std::move(entry_b),
    };

    test_dir().SetEntries(std::move(entries));

    // Verify that we can open all the entries.
    fd = fbl::unique_fd(openat(root.get(), "entry_a", O_DIRECTORY));
    ASSERT_TRUE(fd) << strerror(errno);
    fd = fbl::unique_fd(openat(root.get(), "entry_b", O_DIRECTORY));
    ASSERT_TRUE(fd) << strerror(errno);
    fd = fbl::unique_fd(openat(root.get(), "entry_b/subentry", O_DIRECTORY));
    ASSERT_TRUE(fd) << strerror(errno);

    // Now clear all entries and ensure we cannot open any.
    test_dir().SetEntries({});

    fd = fbl::unique_fd(openat(root.get(), "entry_a", O_DIRECTORY));
    ASSERT_FALSE(fd) << "Opened non-existing entry!";
    ASSERT_EQ(errno, ENOENT) << strerror(errno);
    fd = fbl::unique_fd(openat(root.get(), "entry_b", O_DIRECTORY));
    ASSERT_FALSE(fd) << "Opened non-existing entry!";
    ASSERT_EQ(errno, ENOENT) << strerror(errno);
    fd = fbl::unique_fd(openat(root.get(), "entry_b/subentry", O_DIRECTORY));
    ASSERT_FALSE(fd) << "Opened non-existing entry!";
    ASSERT_EQ(errno, ENOENT) << strerror(errno);
  });
}

}  // namespace
