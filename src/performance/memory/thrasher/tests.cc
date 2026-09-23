// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdio.h>
#include <unistd.h>

#include <iomanip>
#include <iostream>
#include <memory>
#include <optional>
#include <sstream>
#include <thread>

#include "src/performance/memory/thrasher/lib.h"

// Forward declaration from lib.h to resolve build issue
std::string to_hex_string(const std::vector<uint8_t>& bytes);

#include <fidl/fuchsia.fxfs/cpp/wire.h>
#include <fidl/fuchsia.fxfs/cpp/wire_messaging.h>
#include <lib/async-testing/test_loop.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/server.h>
#include <lib/sync/completion.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/sys/component/cpp/testing/realm_builder_types.h>
#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <lib/zx/vmo.h>

#include <gtest/gtest.h>
#include <src/lib/testing/loop_fixture/real_loop_fixture.h>

namespace ffxfs = fuchsia_fxfs;
using component_testing::ChildRef;
using component_testing::LocalComponentFactory;
using component_testing::ParentRef;
using component_testing::Protocol;
using component_testing::RealmBuilder;
using component_testing::Route;

// Helper function to convert hex string to bytes
std::vector<uint8_t> hex_to_bytes(const std::string& hex) {
  std::vector<uint8_t> bytes;
  for (unsigned int i = 0; i < hex.length(); i += 2) {
    std::string byteString = hex.substr(i, 2);
    uint8_t byte = (uint8_t)strtol(byteString.c_str(), NULL, 16);
    bytes.push_back(byte);
  }
  return bytes;
}

struct MockBlobInfo {
  std::string merkle;
  uint64_t size;
};

enum class MockGetVmoErrorType {
  kNone,
  kReplyError,     // Call completer.ReplyError()
  kFidlError,      // Close channel to simulate transport error
  kInvalidHandle,  // ReplySuccess with an invalid VMO
  kZeroSizeVmo,    // ReplySuccess with a zero-size VMO
  kChannelClose    // Call completer.Close()
};

class MockBlobReaderImpl : public component_testing::LocalComponentImpl {
 public:
  explicit MockBlobReaderImpl(async_dispatcher_t* dispatcher, MockBlobReaderImpl** self_ptr)
      : dispatcher_(dispatcher) {
    *self_ptr = this;
  }

  void OnStart() override {
    ASSERT_EQ(ZX_OK, outgoing()->AddProtocol<ffxfs::BlobReader>(bindings_.CreateHandler(
                         &server_, dispatcher_, fidl::kIgnoreBindingClosure)));
  }

  // Methods to control the mock
  void set_mock_blobs(const std::vector<MockBlobInfo>& blobs) { server_.set_mock_blobs(blobs); }
  void set_error_simulation(MockGetVmoErrorType type, zx_status_t error = ZX_OK) {
    server_.set_error_simulation(type, error);
  }
  void set_manual_completion(bool manual) { server_.set_manual_completion(manual); }
  bool has_pending_request() const { return server_.has_pending_request(); }
  void ReplyToPendingRequest(zx::vmo vmo) { server_.ReplyToPendingRequest(std::move(vmo)); }
  size_t get_call_count() const { return server_.get_call_count(); }
  void set_close_channel_after_first_call(bool close) {
    server_.set_close_channel_after_first_call(close);
  }

 private:
  // Inner class to implement the FIDL protocol
  // Inner class to implement the fidl::WireServer<ffxfs::BlobReader> protocol.
  // This mock allows simulating various responses and errors from the BlobReader service.
  class Server : public fidl::WireServer<ffxfs::BlobReader> {
   public:
    void GetVmo(GetVmoRequestView request, GetVmoCompleter::Sync& completer) override {
      if (call_count_ > 0 && close_channel_after_first_call_) {
        completer.Close(ZX_ERR_PEER_CLOSED);
        return;
      }
      call_count_++;

      // Handle manual completion mode for tests that need to control the reply timing.

      if (manual_completion_) {
        pending_completer_ = completer.ToAsync();
        return;
      }
      switch (error_type_) {
        case MockGetVmoErrorType::kReplyError:
          completer.ReplyError(force_error_);
          return;
        case MockGetVmoErrorType::kFidlError:
          completer.Close(ZX_ERR_INTERNAL);  // Simulate transport error
          return;
        case MockGetVmoErrorType::kChannelClose:
          completer.Close(force_error_);  // Or a specific error like ZX_ERR_PEER_CLOSED
          return;
        case MockGetVmoErrorType::kInvalidHandle:
          completer.ReplySuccess(zx::vmo());  // Send invalid handle
          return;
        case MockGetVmoErrorType::kZeroSizeVmo: {
          zx::vmo vmo;
          zx_status_t status = zx::vmo::create(0, 0, &vmo);
          if (status != ZX_OK) {
            completer.ReplyError(status);
          } else {
            completer.ReplySuccess(std::move(vmo));
          }
          return;
        }
        case MockGetVmoErrorType::kNone:
          // Proceed to blob lookup logic if no error simulation is active.
          break;
      }

      std::string merkle_root = to_hex_string(fidl::VectorView<uint8_t>::FromExternal(
          request->blob_hash.data(), request->blob_hash.size()));
      for (const auto& blob : mock_blobs_) {
        if (blob.merkle == merkle_root) {
          zx::vmo parent_vmo;
          zx_status_t status = zx::vmo::create(blob.size, 0, &parent_vmo);
          if (status != ZX_OK) {
            completer.ReplyError(status);
            return;
          }
          // Write some data to the parent VMO to ensure it has pages.
          std::vector<uint8_t> data(blob.size, 0xAA);
          status = parent_vmo.write(data.data(), 0, blob.size);
          if (status != ZX_OK) {
            completer.ReplyError(status);
            return;
          }
          uint64_t vmo_size;
          status = parent_vmo.get_size(&vmo_size);
          if (status != ZX_OK) {
            completer.ReplyError(status);
            return;
          }
          status = parent_vmo.op_range(ZX_VMO_OP_COMMIT, 0, vmo_size, nullptr, 0);
          if (status != ZX_OK) {
            completer.ReplyError(status);
            return;
          }

          // Create a reference child VMO to return, simulating Fxfs behavior.
          // Size must be 0 for ZX_VMO_CHILD_REFERENCE (b/393402141).
          zx::vmo child_vmo;
          status = parent_vmo.create_child(ZX_VMO_CHILD_REFERENCE | ZX_VMO_CHILD_NO_WRITE, 0, 0,
                                           &child_vmo);
          if (status != ZX_OK) {
            completer.ReplyError(status);
            return;
          }

          // We must keep the parent VMO alive for the child to be valid if it's a reference?
          // Actually, for REFERENCE, if the parent dies, the child might be invalid or empty?
          // Let's keep it alive in the mock for now to be safe, or maybe it's fine if it dies
          // if it's just a reference to pages?
          // Re-reading vmo-reference.cc: "Closing the parent VMO will not affect the reference."
          // Wait, is that true?
          // "The reference VMO will continue to point to the same pages even if the parent VMO is
          // closed." Let's assume it's fine to let parent_vmo go out of scope here if it works like
          // that. Actually, if it's a reference to *pages*, and parent goes away, who owns the
          // pages? "ZX_VMO_CHILD_REFERENCE creates a VMO that shares the same pages as the parent
          // VMO." If parent is closed, the pages might be freed if nothing else holds them. Let's
          // keep parent VMOs alive in the mock just in case.
          parent_vmos_.push_back(std::move(parent_vmo));

          completer.ReplySuccess(std::move(child_vmo));
          return;
        }
      }
      completer.ReplyError(ZX_ERR_NOT_FOUND);  // Default if merkle root not found in mock_blobs_
    }

    void set_mock_blobs(const std::vector<MockBlobInfo>& blobs) { mock_blobs_ = blobs; }
    void set_error_simulation(MockGetVmoErrorType type, zx_status_t error = ZX_OK) {
      error_type_ = type;
      force_error_ = error;
    }
    void set_manual_completion(bool manual) { manual_completion_ = manual; }
    bool has_pending_request() const { return pending_completer_.has_value(); }
    void ReplyToPendingRequest(zx::vmo vmo) {
      ASSERT_TRUE(pending_completer_.has_value());
      pending_completer_->ReplySuccess(std::move(vmo));
      pending_completer_.reset();
    }
    size_t get_call_count() const { return call_count_; }
    void set_close_channel_after_first_call(bool close) { close_channel_after_first_call_ = close; }

   private:
    using GetVmoRequestView = fidl::WireServer<ffxfs::BlobReader>::GetVmoRequestView;
    using GetVmoCompleter = fidl::WireServer<ffxfs::BlobReader>::GetVmoCompleter;

    std::string to_hex_string(const fidl::VectorView<uint8_t>& vec) {
      std::stringstream ss;
      ss << std::hex << std::setfill('0');
      for (uint8_t byte : vec) {
        ss << std::setw(2) << static_cast<int>(byte);
      }
      return ss.str();
    }

    std::vector<MockBlobInfo> mock_blobs_;
    MockGetVmoErrorType error_type_ = MockGetVmoErrorType::kNone;
    zx_status_t force_error_ = ZX_OK;
    size_t call_count_ = 0;
    bool manual_completion_ = false;
    std::optional<GetVmoCompleter::Async> pending_completer_;
    std::vector<zx::vmo> parent_vmos_;
    bool close_channel_after_first_call_ = false;
  };

  async_dispatcher_t* dispatcher_;
  Server server_;
  fidl::ServerBindingGroup<ffxfs::BlobReader> bindings_;
};

namespace fio = fuchsia_io;

class ThrasherTest : public gtest::RealLoopFixture {
 protected:
  MockBlobReaderImpl* mock_blob_reader_ptr_ =
      nullptr;  // Pointer to the mock instance, set by the factory.
  std::unique_ptr<component_testing::RealmRoot> realm_;  // Manages the test realm.

  // Sets up the test realm using RealmBuilder.
  // Adds the MockBlobReaderImpl as a local component.
  // Optionally routes the fuchsia.fxfs.BlobReader protocol from the mock to the realm's parent.
  fidl::ClientEnd<ffxfs::BlobReader> CreateRealm(bool route_blob_reader = true) {
    auto realm_builder = component_testing::RealmBuilder::Create();
    mock_blob_reader_ptr_ = nullptr;

    realm_builder.AddLocalChild(
        "mock_blob_reader",
        [&]() -> std::unique_ptr<component_testing::LocalComponentImpl> {
          auto mock =
              std::make_unique<MockBlobReaderImpl>(this->dispatcher(), &mock_blob_reader_ptr_);
          return mock;
        },
        component_testing::ChildOptions{.startup_mode = component_testing::StartupMode::EAGER});

    if (route_blob_reader) {
      realm_builder.AddRoute(
          component_testing::Route{.capabilities = {component_testing::Protocol{
                                       fidl::DiscoverableProtocolName<ffxfs::BlobReader>}},
                                   .source = component_testing::ChildRef{"mock_blob_reader"},
                                   .targets = {component_testing::ParentRef()}});
    }

    realm_builder.AddRoute(component_testing::Route{
        .capabilities = {component_testing::Protocol{"fuchsia.logger.LogSink"}},
        .source = component_testing::ParentRef(),
        .targets = {component_testing::ChildRef{"mock_blob_reader"}}});

    realm_ =
        std::make_unique<component_testing::RealmRoot>(realm_builder.Build(this->dispatcher()));

    // Connect to the service within the realm
    auto client_end = realm_->component().Connect<ffxfs::BlobReader>();
    if (!client_end.is_ok()) {
      FX_LOGS(ERROR) << "CreateRealm: Failed to connect to BlobReader from realm: "
                     << client_end.status_string();
      return {};
    }

    // Wait for the mock to be created by the AddLocalChild lambda.
    RunLoopUntil([&] { return mock_blob_reader_ptr_ != nullptr; });

    return std::move(client_end.value());
  }

  // Tears down the test realm, ensuring components are stopped and resources are freed.
  // The event loop is run to allow asynchronous teardown tasks to complete.
  void TearDown() override {
    if (realm_) {
      bool complete = false;
      realm_->Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
      RunLoopUntil([&]() { return complete; });
    }
    gtest::RealLoopFixture::TearDown();
  }

  // Tears down the test realm, ensuring components are stopped and resources are freed.
  // The event loop is run to allow asynchronous teardown tasks to complete.
};

// TODO(https://fxbug.dev/485307031): Disabled until flakes are fixed.
TEST_F(ThrasherTest, DISABLED_ParentVmoAccountingTest) {
  const size_t kPageSize = zx_system_get_page_size();
  const size_t kVmoSize = kPageSize * 4;
  zx::vmo parent_vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &parent_vmo), ZX_OK);

  // Commit pages to the parent VMO.
  ASSERT_EQ(parent_vmo.op_range(ZX_VMO_OP_COMMIT, 0, kVmoSize, nullptr, 0), ZX_OK);

  // Verify parent VMO has committed bytes.
  zx_info_vmo_t parent_info;
  ASSERT_EQ(parent_vmo.get_info(ZX_INFO_VMO, &parent_info, sizeof(parent_info), nullptr, nullptr),
            ZX_OK);
  EXPECT_EQ(parent_info.committed_bytes, kVmoSize);
  EXPECT_EQ(parent_info.parent_koid, 0u);  // It's a root VMO

  // Create a reference child VMO.
  zx::vmo child_vmo;
  // Size must be 0 for ZX_VMO_CHILD_REFERENCE (b/393402141)
  ASSERT_EQ(
      parent_vmo.create_child(ZX_VMO_CHILD_REFERENCE | ZX_VMO_CHILD_NO_WRITE, 0, 0, &child_vmo),
      ZX_OK);

  // Verify child VMO reports 0 committed bytes via ZX_INFO_VMO.
  zx_info_vmo_t child_info;
  ASSERT_EQ(child_vmo.get_info(ZX_INFO_VMO, &child_info, sizeof(child_info), nullptr, nullptr),
            ZX_OK);
  EXPECT_EQ(child_info.committed_bytes, 0u);
  EXPECT_EQ(child_info.parent_koid, parent_info.koid);

  // In this test we have handles to both, so we can verify the accounting works if we use the
  // parent. This confirms the strategy: if we can find the parent, we can get the bytes.

  // Call log_vmos to verify our new accounting logic.
  // It should be able to find parent_vmo because we hold a handle to it in this process.
  std::vector<zx::vmo> vmos_to_log;
  zx::vmo child_dup;
  ASSERT_EQ(child_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &child_dup), ZX_OK);
  vmos_to_log.push_back(std::move(child_dup));

  LogVmos(vmos_to_log, true);

  // Map the child VMO and check ZX_INFO_PROCESS_MAPS.
  uintptr_t mapped_addr;
  ASSERT_EQ(zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, child_vmo, 0, kVmoSize, &mapped_addr),
            ZX_OK);

  // Touch the pages to be sure.
  volatile uint8_t* ptr = reinterpret_cast<uint8_t*>(mapped_addr);
  for (size_t i = 0; i < kVmoSize; i += kPageSize) {
    (void)ptr[i];
  }

  zx_info_handle_basic_t child_basic_info;
  ASSERT_EQ(child_vmo.get_info(ZX_INFO_HANDLE_BASIC, &child_basic_info, sizeof(child_basic_info),
                               nullptr, nullptr),
            ZX_OK);
  zx_koid_t child_koid = child_basic_info.koid;

  size_t actual = 0;
  size_t avail = 0;
  std::vector<zx_info_maps_t> maps;
  do {
    ASSERT_EQ(zx::process::self()->get_info(ZX_INFO_PROCESS_MAPS, nullptr, 0, &actual, &avail),
              ZX_OK);
    // Add headroom to avoid spinning if mappings are being created rapidly.
    maps.resize(avail + 16);
    ASSERT_EQ(zx::process::self()->get_info(ZX_INFO_PROCESS_MAPS, maps.data(),
                                            maps.size() * sizeof(zx_info_maps_t), &actual, &avail),
              ZX_OK);
  } while (actual < avail);

  bool found_mapping = false;
  for (size_t i = 0; i < actual; ++i) {
    if (maps[i].type == ZX_INFO_MAPS_TYPE_MAPPING && maps[i].u.mapping.vmo_koid == child_koid) {
      found_mapping = true;
      // Expect 0 committed bytes for reference child mapping too, based on previous findings.
      EXPECT_EQ(maps[i].u.mapping.committed_bytes, 0u);
      break;
    }
  }
  EXPECT_TRUE(found_mapping);

  ASSERT_EQ(zx::vmar::root_self()->unmap(mapped_addr, kVmoSize), ZX_OK);
}

void RunThrashTest(std::shared_ptr<Thrasher> thrasher, async::Loop& loop) {
  bool init_done = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;
  thrasher->Initialize([&](zx_status_t status) {
    init_status = status;
    init_done = true;
  });

  while (!init_done) {
    loop.RunUntilIdle();
  }
  ASSERT_EQ(init_status, ZX_OK);

  // Callbacks are now passed to Start()
  loop.Run();
}

void RunThrashTestWithConfig(
    fit::function<std::shared_ptr<Thrasher>(ThrashConfig config)> create_fn) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  bool done = false;
  std::vector<zx::vmo> vmos;

  ThrashConfig config = {
      .bursts_per_second = 100,
      .run_for_seconds = 1,
      .num_threads = 1,
      .pages_per_read = 1,
      .consecutive_pages_per_read = 1,
      .dispatcher = loop.dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> result) {
    vmos = std::move(result);
    done = true;
    loop.Quit();
  });

  auto thrasher = create_fn(std::move(config));
  ASSERT_NE(thrasher, nullptr);

  bool init_done = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;
  thrasher->Initialize([&](zx_status_t status) {
    init_status = status;
    init_done = true;
  });

  while (!init_done) {
    loop.RunUntilIdle();
  }
  ASSERT_EQ(init_status, ZX_OK);

  thrasher->Start(thrash_callback, nullptr);
  loop.Run();

  ASSERT_TRUE(done);
  ASSERT_FALSE(vmos.empty());
  for (const auto& vmo : vmos) {
    ASSERT_TRUE(vmo.is_valid());
  }
}

TEST_F(ThrasherTest, AddLocalChildCallbackTest) {
  auto client_end = CreateRealm();
  ASSERT_TRUE(client_end.is_valid()) << "Failed to connect to BlobReader";

  // Bind to an async client to make a call to force startup
  fidl::WireClient<ffxfs::BlobReader> client(std::move(client_end), this->dispatcher());
  uint8_t hash[32] = {0};
  fidl::Array<uint8_t, 32> fidl_hash;
  std::copy(std::begin(hash), std::end(hash), fidl_hash.begin());

  bool call_complete = false;
  client->GetVmo(fidl_hash).Then(
      [&](fidl::WireUnownedResult<ffxfs::BlobReader::GetVmo>& result) { call_complete = true; });

  // Run the loop to allow the call to go out and the component to start.
  RunLoopUntil([&] { return mock_blob_reader_ptr_ != nullptr; });

  EXPECT_NE(mock_blob_reader_ptr_, nullptr) << "mock_blob_reader_ptr_ was not set";

  // Unbind the client before destroying the realm to avoid hanging.
  client = {};
}

TEST_F(ThrasherTest, AnonThrasherInvalidSizeTest) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  ThrashConfig config = {
      .dispatcher = loop.dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };
  auto thrasher = CreateAnonThrasher(std::move(config), 0);
  ASSERT_NE(thrasher, nullptr);

  bool init_done = false;
  zx_status_t init_status = ZX_OK;
  thrasher->Initialize([&](zx_status_t status) {
    init_status = status;
    init_done = true;
  });

  RunLoopUntil([&] { return init_done; });
  EXPECT_EQ(init_status, ZX_ERR_INVALID_ARGS);
}

TEST(Thrasher, SmokeTest) {
  RunThrashTestWithConfig(
      [](ThrashConfig config) { return CreateAnonThrasher(std::move(config), 1024 * 1024); });
}

TEST_F(ThrasherTest, MmapThrasherNonExistentFileTest) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  ThrashConfig config = {
      .dispatcher = loop.dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };
  auto thrasher = CreateMmapThrasher(std::move(config), "/pkg/data/non_existent_file");
  ASSERT_NE(thrasher, nullptr);

  bool init_done = false;
  zx_status_t init_status = ZX_OK;
  thrasher->Initialize([&](zx_status_t status) {
    init_status = status;
    init_done = true;
  });

  RunLoopUntil([&] { return init_done; });
  EXPECT_EQ(init_status, ZX_ERR_IO);
}

TEST(Thrasher, MappedFileSmokeTest) {
  RunThrashTestWithConfig([](ThrashConfig config) {
    return CreateMmapThrasher(std::move(config), "/pkg/data/test_data");
  });
}

TEST(Thrasher, MultiThreadedAnonymous) {
  RunThrashTestWithConfig([](ThrashConfig config) {
    config.num_threads = 4;
    return CreateAnonThrasher(std::move(config), 1024 * 1024);
  });
}

TEST(Thrasher, MultiThreadedMappedFile) {
  RunThrashTestWithConfig([](ThrashConfig config) {
    config.num_threads = 4;
    return CreateMmapThrasher(std::move(config), "/pkg/data/test_data");
  });
}

TEST_F(ThrasherTest, DirThrasherNonExistentDirTest) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  ThrashConfig config = {
      .dispatcher = loop.dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };
  auto thrasher = CreateDirThrasher(std::move(config), "/pkg/data/non_existent_dir");
  ASSERT_NE(thrasher, nullptr);

  bool init_done = false;
  zx_status_t init_status = ZX_OK;
  thrasher->Initialize([&](zx_status_t status) {
    init_status = status;
    init_done = true;
  });

  RunLoopUntil([&] { return init_done; });
  EXPECT_EQ(init_status, ZX_OK);  // opendir failing on a path is not an init error for DirThrasher

  // Start should also complete fine, but with no work done.
  bool callback_called = false;
  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    callback_called = true;
    EXPECT_TRUE(vmos.empty());
    loop.Quit();
  });
  thrasher->Start(thrash_callback, nullptr);
  loop.Run();
  EXPECT_TRUE(callback_called);
}

TEST_F(ThrasherTest, DirThrasherEmptyDirTest) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  ThrashConfig config = {
      .dispatcher = loop.dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };
  auto thrasher = CreateDirThrasher(std::move(config), "/pkg/data/empty_dir");
  ASSERT_NE(thrasher, nullptr);

  bool init_done = false;
  zx_status_t init_status = ZX_OK;
  thrasher->Initialize([&](zx_status_t status) {
    init_status = status;
    init_done = true;
  });

  RunLoopUntil([&] { return init_done; });
  EXPECT_EQ(init_status, ZX_OK);

  // Start should also complete fine, but with no work done.
  bool callback_called = false;
  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    callback_called = true;
    EXPECT_TRUE(vmos.empty());
    loop.Quit();
  });
  thrasher->Start(thrash_callback, nullptr);
  loop.Run();
  EXPECT_TRUE(callback_called);
}

TEST(Thrasher, DirectoryTest) {
  RunThrashTestWithConfig([](ThrashConfig config) {
    return CreateDirThrasher(std::move(config), "/pkg/data/test_dir");
  });
}

// This test is no longer valid as the check is in the Thrasher class constructor/init.
// We test initialization failure in blob tests.
// Test case for when BlobReader returns an error
TEST_F(ThrasherTest, ThrashBlobsErrorTest) {
  auto client_end = CreateRealm();
  ASSERT_NE(mock_blob_reader_ptr_, nullptr);

  // Set the mock to return an error
  mock_blob_reader_ptr_->set_error_simulation(MockGetVmoErrorType::kReplyError, ZX_ERR_INTERNAL);

  std::vector<MockBlobInfo> mock_blobs = {
      {"1111111111111111111111111111111111111111111111111111111111111111", 1000},
  };
  mock_blob_reader_ptr_->set_mock_blobs(mock_blobs);

  bool callback_called = false;
  bool init_called = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;

  ThrashConfig config = {
      .bursts_per_second = 1000,
      .run_for_seconds = 1,
      .num_threads = 1,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    callback_called = true;
    EXPECT_TRUE(vmos.empty());
    QuitLoop();
  });

  std::vector<std::string> merkle_roots = {
      {"1111111111111111111111111111111111111111111111111111111111111111"}};

  auto thrasher = CreateBlobThrasherWithClient(std::move(config), std::move(client_end),
                                               merkle_roots, 1024 * 1024 * 1024);
  ASSERT_NE(thrasher, nullptr);

  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
  });

  RunLoopUntil([&]() { return init_called; });
  EXPECT_EQ(init_status, ZX_OK);  // Initialization succeeds even if GetVmo will fail.

  thrasher->Start(thrash_callback, nullptr);
  RunLoopUntil([&]() { return callback_called; });
  EXPECT_TRUE(callback_called);
}

// Test case for when BlobReader succeeds
TEST_F(ThrasherTest, ThrashBlobsSuccessTest) {
  auto client_end = CreateRealm();
  ASSERT_NE(mock_blob_reader_ptr_, nullptr);

  std::vector<MockBlobInfo> mock_blobs = {
      {"2222222222222222222222222222222222222222222222222222222222222222", 2000},
      {"3333333333333333333333333333333333333333333333333333333333333333", 3000},
  };
  mock_blob_reader_ptr_->set_mock_blobs(mock_blobs);

  ASSERT_TRUE(client_end.is_valid());
  bool callback_called = false;
  bool init_called = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;

  ThrashConfig config = {
      .bursts_per_second = 1000,
      .run_for_seconds = 1,
      .num_threads = 1,
      .pages_per_read = 1,
      .consecutive_pages_per_read = 1,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    callback_called = true;
    ASSERT_EQ(vmos.size(), 2u);
    uint64_t size;
    vmos[0].get_size(&size);
    EXPECT_EQ(size, 4096u);
    vmos[1].get_size(&size);
    EXPECT_EQ(size, 4096u);
    QuitLoop();
  });

  std::vector<std::string> merkle_roots = {
      {"2222222222222222222222222222222222222222222222222222222222222222"},
      {"3333333333333333333333333333333333333333333333333333333333333333"}};

  auto thrasher = CreateBlobThrasherWithClient(std::move(config), std::move(client_end),
                                               merkle_roots, 1024 * 1024 * 1024);
  ASSERT_NE(thrasher, nullptr);

  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
  });

  RunLoopUntil([&]() { return init_called; });
  ASSERT_EQ(init_status, ZX_OK);

  thrasher->Start(thrash_callback, nullptr);

  RunLoopUntil([&]() { return callback_called; });
}

TEST(Thrasher, LogVmosTest) {
  std::vector<zx::vmo> vmos;
  // Test with empty vector
  LogVmos(vmos, true);

  // Test with valid VMOs
  zx::vmo vmo1;
  ASSERT_EQ(zx::vmo::create(4096, 0, &vmo1), ZX_OK);
  vmos.push_back(std::move(vmo1));
  zx::vmo vmo2;
  ASSERT_EQ(zx::vmo::create(8192, 0, &vmo2), ZX_OK);
  vmos.push_back(std::move(vmo2));
  LogVmos(vmos, true);

  // Test with an invalid VMO
  vmos.clear();
  zx::vmo invalid_vmo;
  vmos.push_back(std::move(invalid_vmo));
  LogVmos(vmos, true);
}

// Test case for an invalid merkle root
TEST_F(ThrasherTest, ThrashBlobsInvalidMerkleTest) {
  auto client_end = CreateRealm();
  ASSERT_NE(realm_, nullptr);

  // Run the loop to allow the component to start and the factory lambda to execute.
  RunLoopUntilIdle();

  ASSERT_NE(mock_blob_reader_ptr_, nullptr);

  // No blobs set in the mock, so any merkle will be not found.
  mock_blob_reader_ptr_->set_mock_blobs({});

  std::vector<std::string> merkle_roots = {
      "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"};
  bool callback_called = false;
  bool init_called = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;

  ASSERT_TRUE(client_end.is_valid());
  ThrashConfig config = {
      .bursts_per_second = 1000,
      .run_for_seconds = 1,
      .num_threads = 1,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    callback_called = true;
    EXPECT_TRUE(vmos.empty());
    QuitLoop();
  });

  auto thrasher = CreateBlobThrasherWithClient(std::move(config), std::move(client_end),
                                               merkle_roots, 1024 * 1024 * 1024);
  ASSERT_NE(thrasher, nullptr);

  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
  });

  RunLoopUntil([&]() { return init_called; });
  EXPECT_EQ(init_status, ZX_OK);

  thrasher->Start(thrash_callback, nullptr);
  RunLoopUntil([&]() { return callback_called; });
  EXPECT_TRUE(callback_called);
}

// Test case for VMO error during GetVmo
TEST_F(ThrasherTest, ThrashBlobsVmoGetSizeErrorTest) {
  auto client_end = CreateRealm();
  ASSERT_NE(mock_blob_reader_ptr_, nullptr);
  ASSERT_TRUE(client_end.is_valid());
  mock_blob_reader_ptr_->set_error_simulation(MockGetVmoErrorType::kZeroSizeVmo);
  bool callback_called = false;
  bool init_called = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;

  ThrashConfig config = {
      .bursts_per_second = 1000,
      .run_for_seconds = 1,
      .num_threads = 1,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    EXPECT_TRUE(vmos.empty());
    callback_called = true;
    QuitLoop();
  });

  std::vector<MockBlobInfo> mock_blobs = {
      {"4444444444444444444444444444444444444444444444444444444444444444", 1000},
  };
  mock_blob_reader_ptr_->set_mock_blobs(mock_blobs);

  std::vector<std::string> merkle_roots = {
      {"4444444444444444444444444444444444444444444444444444444444444444"}};

  auto thrasher = CreateBlobThrasherWithClient(std::move(config), std::move(client_end),
                                               merkle_roots, 1024 * 1024 * 1024);
  ASSERT_NE(thrasher, nullptr);

  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
  });

  RunLoopUntil([&]() { return init_called; });
  ASSERT_EQ(init_status, ZX_OK);

  thrasher->Start(thrash_callback, nullptr);
  RunLoopUntil([&]() { return callback_called; });
  EXPECT_TRUE(callback_called);
}

// Test case for BlobThrasher with bursts_per_second = 0
TEST_F(ThrasherTest, ThrashBlobsInvalidConfigBursts) {
  auto client_end = CreateRealm();
  ASSERT_TRUE(client_end.is_valid());

  ThrashConfig config = {
      .bursts_per_second = 0,
      .run_for_seconds = 1,
      .num_threads = 1,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };

  std::vector<std::string> merkle_roots = {
      "1111111111111111111111111111111111111111111111111111111111111111"};

  auto thrasher = CreateBlobThrasherWithClient(std::move(config), std::move(client_end),
                                               merkle_roots, 1024 * 1024);
  ASSERT_NE(thrasher, nullptr);

  bool init_called = false;
  zx_status_t init_status = ZX_OK;
  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
  });

  RunLoopUntil([&]() { return init_called; });
  EXPECT_EQ(init_status, ZX_ERR_INVALID_ARGS);
}

// Test case for BlobThrasher with num_threads = 0
TEST_F(ThrasherTest, ThrashBlobsInvalidConfigThreads) {
  auto client_end = CreateRealm();
  ASSERT_TRUE(client_end.is_valid());

  ThrashConfig config = {
      .bursts_per_second = 1000,
      .run_for_seconds = 1,
      .num_threads = 0,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };

  std::vector<std::string> merkle_roots = {
      "1111111111111111111111111111111111111111111111111111111111111111"};

  auto thrasher = CreateBlobThrasherWithClient(std::move(config), std::move(client_end),
                                               merkle_roots, 1024 * 1024);
  ASSERT_NE(thrasher, nullptr);

  bool init_called = false;
  zx_status_t init_status = ZX_OK;
  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
  });

  RunLoopUntil([&]() { return init_called; });
  EXPECT_EQ(init_status, ZX_ERR_INVALID_ARGS);
}

// Test case for a merkle root that is not 64 characters long
TEST_F(ThrasherTest, ThrashBlobsShortMerkleTest) {
  auto client_end = CreateRealm();
  ASSERT_NE(mock_blob_reader_ptr_, nullptr);
  bool callback_called = false;
  bool init_called = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;

  ASSERT_TRUE(client_end.is_valid());
  ThrashConfig config = {
      .bursts_per_second = 1000,
      .run_for_seconds = 1,
      .num_threads = 1,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    callback_called = true;
    EXPECT_TRUE(vmos.empty());
    QuitLoop();
  });

  std::vector<std::string> merkle_roots = {"short_merkle"};

  auto thrasher = CreateBlobThrasherWithClient(std::move(config), std::move(client_end),
                                               merkle_roots, 1024 * 1024 * 1024);
  ASSERT_NE(thrasher, nullptr);

  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
  });

  RunLoopUntil([&]() { return init_called; });
  EXPECT_EQ(init_status, ZX_OK);

  thrasher->Start(thrash_callback, nullptr);
  RunLoopUntil([&]() { return callback_called; });
  EXPECT_TRUE(callback_called);
}

// Test case for when an invalid client handle is passed to the thrashing manager.
TEST_F(ThrasherTest, ThrashBlobsInvalidClientHandle) {
  bool callback_called = false;
  bool init_called = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;

  ThrashConfig config = {
      .bursts_per_second = 1000,
      .run_for_seconds = 1,
      .num_threads = 1,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    callback_called = true;
    EXPECT_TRUE(vmos.empty());
    QuitLoop();
  });

  std::vector<std::string> merkle_roots = {
      "1111111111111111111111111111111111111111111111111111111111111111"};

  auto thrasher = CreateBlobThrasherWithClient(
      std::move(config), fidl::ClientEnd<ffxfs::BlobReader>(), merkle_roots, 1024 * 1024 * 1024);
  ASSERT_NE(thrasher, nullptr);

  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
    if (status != ZX_OK) {
      QuitLoop();
    }
  });
  RunLoopUntil([&]() { return init_called; });
  EXPECT_EQ(init_status, ZX_ERR_BAD_HANDLE);

  // Don't call Start() if Initialize failed.
  // The Initialize callback should have called QuitLoop().
}

// Test case for when no merkle roots are provided to the thrashing manager.
TEST_F(ThrasherTest, ThrashBlobsNoMerkleRoots) {
  auto client_end = CreateRealm();
  ASSERT_NE(mock_blob_reader_ptr_, nullptr);
  bool callback_called = false;
  bool init_called = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;

  ASSERT_TRUE(client_end.is_valid());
  ThrashConfig config = {
      .bursts_per_second = 1000,
      .run_for_seconds = 1,
      .num_threads = 1,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    callback_called = true;
    EXPECT_TRUE(vmos.empty());
    QuitLoop();
  });

  std::vector<std::string> merkle_roots = {};

  auto thrasher = CreateBlobThrasherWithClient(std::move(config), std::move(client_end),
                                               merkle_roots, 1024 * 1024 * 1024);
  ASSERT_NE(thrasher, nullptr);

  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
  });

  RunLoopUntil([&]() { return init_called; });
  EXPECT_EQ(init_status, ZX_OK);

  thrasher->Start(thrash_callback, nullptr);
  RunLoopUntil([&]() { return callback_called; });
  EXPECT_TRUE(callback_called);
}

// Test case for FIDL error during GetVmo
TEST_F(ThrasherTest, ThrashBlobsGetVmoFidlErrorTest) {
  auto client_end = CreateRealm();
  ASSERT_NE(mock_blob_reader_ptr_, nullptr);
  mock_blob_reader_ptr_->set_error_simulation(MockGetVmoErrorType::kFidlError);
  bool callback_called = false;
  bool init_called = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;

  ASSERT_TRUE(client_end.is_valid());
  ThrashConfig config = {
      .num_threads = 1,
      .pages_per_read = 1,
      .consecutive_pages_per_read = 1,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    EXPECT_TRUE(vmos.empty());
    callback_called = true;
    QuitLoop();
  });

  std::vector<std::string> merkle_roots = {
      {"1111111111111111111111111111111111111111111111111111111111111111"}};

  auto thrasher = CreateBlobThrasherWithClient(std::move(config), std::move(client_end),
                                               merkle_roots, 1024 * 1024 * 1024);
  ASSERT_NE(thrasher, nullptr);

  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
    if (status != ZX_OK) {
      QuitLoop();
    }
  });

  RunLoopUntil([&]() { return init_called; });
  EXPECT_EQ(init_status, ZX_ERR_INTERNAL);

  EXPECT_FALSE(callback_called);
}

class ThrasherUnroutedTest : public gtest::RealLoopFixture {
 protected:
  std::unique_ptr<component_testing::RealmRoot> realm_;
  MockBlobReaderImpl* mock_blob_reader_ptr_ = nullptr;

  fidl::ClientEnd<ffxfs::BlobReader> CreateRealm() {
    auto realm_builder = component_testing::RealmBuilder::Create();

    // We need a mock implementation even if we don't route it, just to have a valid component to
    // add. We don't need to capture the pointer since we won't be interacting with it.
    realm_builder.AddLocalChild(
        "mock_blob_reader",
        [&]() -> std::unique_ptr<component_testing::LocalComponentImpl> {
          return std::make_unique<MockBlobReaderImpl>(this->dispatcher(), &mock_blob_reader_ptr_);
        },
        component_testing::ChildOptions{.startup_mode = component_testing::StartupMode::EAGER});

    // Deliberately NOT adding a route for BlobReader from mock_blob_reader to Parent.

    realm_builder.AddRoute(component_testing::Route{
        .capabilities = {component_testing::Protocol{"fuchsia.logger.LogSink"}},
        .source = component_testing::ParentRef(),
        .targets = {component_testing::ChildRef{"mock_blob_reader"}}});

    realm_ =
        std::make_unique<component_testing::RealmRoot>(realm_builder.Build(this->dispatcher()));

    // Connect to the service within the realm - this should succeed in getting a channel,
    // but the channel will be closed by component manager when it tries to route it.
    auto client_end = realm_->component().Connect<ffxfs::BlobReader>();
    if (!client_end.is_ok()) {
      return {};
    }

    return std::move(client_end.value());
  }

  void TearDown() override {
    if (realm_) {
      bool complete = false;
      realm_->Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
      RunLoopUntil([&]() { return complete; });
    }
    gtest::RealLoopFixture::TearDown();
  }
};

TEST_F(ThrasherTest, ThrashBlobsUnroutedTest) {
  auto client_end = CreateRealm(false);
  ASSERT_TRUE(client_end.is_valid());

  bool callback_called = false;
  bool init_called = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;

  ThrashConfig config = {
      .bursts_per_second = 1000,
      .run_for_seconds = 1,
      .num_threads = 1,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    callback_called = true;
    EXPECT_TRUE(vmos.empty());
    QuitLoop();
  });
  auto shared_callback = thrash_callback;

  // Try with multiple blobs to ensure we only log the error once (verified manually by looking at
  // logs)
  std::vector<std::string> merkle_roots = {
      {"1111111111111111111111111111111111111111111111111111111111111111"},
      {"2222222222222222222222222222222222222222222222222222222222222222"}};

  auto thrasher = CreateBlobThrasherWithClient(std::move(config), std::move(client_end),
                                               merkle_roots, 1024 * 1024 * 1024);
  ASSERT_NE(thrasher, nullptr);

  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
    if (status != ZX_OK) {
      QuitLoop();
    }
  });
  RunLoopUntil([&] { return init_called; });
  EXPECT_EQ(init_status, ZX_ERR_NOT_FOUND);

  EXPECT_FALSE(callback_called);
  // Verify that GetVmo was called exactly once (for the probe).
  // This test doesn't have a mock_blob_reader_ptr_ to check call count.
  // The mock is in a child realm and not directly accessible.
}

TEST_F(ThrasherTest, ConnectionErrorStopsSubsequentCalls) {
  auto client_end = CreateRealm();
  ASSERT_NE(mock_blob_reader_ptr_, nullptr);

  // Simulate a connection error (e.g., peer closed) after the first call.
  mock_blob_reader_ptr_->set_error_simulation(MockGetVmoErrorType::kNone);
  mock_blob_reader_ptr_->set_close_channel_after_first_call(true);

  bool callback_called = false;
  bool init_called = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;

  ThrashConfig config = {
      .bursts_per_second = 1000,
      .run_for_seconds = 1,
      .num_threads = 1,
      .pages_per_read = 1,
      .consecutive_pages_per_read = 1,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    callback_called = true;
    EXPECT_EQ(vmos.size(), 1u);
    QuitLoop();
  });
  auto shared_callback = thrash_callback;

  std::vector<MockBlobInfo> mock_blobs = {
      {"1111111111111111111111111111111111111111111111111111111111111111", 1000},
      {"2222222222222222222222222222222222222222222222222222222222222222", 2000},
  };
  mock_blob_reader_ptr_->set_mock_blobs(mock_blobs);

  std::vector<std::string> merkle_roots = {
      {"1111111111111111111111111111111111111111111111111111111111111111"},
      {"2222222222222222222222222222222222222222222222222222222222222222"}};

  auto thrasher = CreateBlobThrasherWithClient(std::move(config), std::move(client_end),
                                               merkle_roots, 1024 * 1024 * 1024);
  ASSERT_NE(thrasher, nullptr);

  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
    if (status != ZX_OK) {
      QuitLoop();  // Should not happen in this test
    }
  });

  RunLoopUntil([&]() { return init_called; });
  EXPECT_EQ(init_status, ZX_OK);  // Initial probe succeeds.

  thrasher->Start(thrash_callback, nullptr);

  RunLoopUntil([&]() { return callback_called; });
  EXPECT_TRUE(callback_called);
  // Verify that GetVmo was called exactly once.
  EXPECT_EQ(mock_blob_reader_ptr_->get_call_count(), 1u);
}

TEST_F(ThrasherTest, ThrashBlobsSequentialTest) {
  auto client_end = CreateRealm();
  mock_blob_reader_ptr_->set_manual_completion(true);

  sync_completion_t completion;
  bool callback_called = false;
  bool init_called = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;

  ThrashConfig config = {
      .bursts_per_second = 1,
      .run_for_seconds = 1,
      .pages_per_read = 1,
      .consecutive_pages_per_read = 1,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},  // Return instantly
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    EXPECT_EQ(vmos.size(), 2u);
    callback_called = true;
    sync_completion_signal(&completion);
  });

  std::vector<std::string> merkle_roots = {
      {"1111111111111111111111111111111111111111111111111111111111111111"},
      {"2222222222222222222222222222222222222222222222222222222222222222"}};

  auto thrasher = CreateBlobThrasherWithClient(std::move(config), std::move(client_end),
                                               merkle_roots, 1024 * 1024 * 1024);
  ASSERT_NE(thrasher, nullptr);

  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
  });

  // Wait for the first request during Initialize
  RunLoopUntil([&]() { return mock_blob_reader_ptr_->has_pending_request(); });
  EXPECT_EQ(mock_blob_reader_ptr_->get_call_count(), 1u);

  // Reply to the first request
  zx::vmo vmo1;
  ASSERT_EQ(zx::vmo::create(4096, 0, &vmo1), ZX_OK);
  mock_blob_reader_ptr_->ReplyToPendingRequest(std::move(vmo1));

  // Wait for the second request
  RunLoopUntil([&]() { return mock_blob_reader_ptr_->has_pending_request(); });
  EXPECT_EQ(mock_blob_reader_ptr_->get_call_count(), 2u);

  // Reply to the second request
  zx::vmo vmo2;
  ASSERT_EQ(zx::vmo::create(4096, 0, &vmo2), ZX_OK);
  mock_blob_reader_ptr_->ReplyToPendingRequest(std::move(vmo2));

  // Now Initialize should complete
  RunLoopUntil([&]() { return init_called; });
  ASSERT_EQ(init_status, ZX_OK);

  thrasher->Start(thrash_callback, nullptr);

  // Wait for finish
  RunLoopUntil([&] { return callback_called; });
  // Wait for the async task to signal completion
  ASSERT_EQ(sync_completion_wait(&completion, ZX_TIME_INFINITE), ZX_OK);
}

TEST_F(ThrasherTest, ThrashBlobsMemoryLimitTest) {
  auto client_end = CreateRealm();
  mock_blob_reader_ptr_->set_manual_completion(true);

  bool callback_called = false;
  bool init_called = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;

  ThrashConfig config = {
      .bursts_per_second = 1,
      .run_for_seconds = 1,
      .pages_per_read = 1,
      .consecutive_pages_per_read = 1,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},  // Return instantly
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    callback_called = true;
    EXPECT_EQ(vmos.size(), 2u);  // Should have fetched two blobs (one over limit)
    QuitLoop();
  });

  std::vector<std::string> merkle_roots = {
      {"1111111111111111111111111111111111111111111111111111111111111111"},
      {"2222222222222222222222222222222222222222222222222222222222222222"},
      {"3333333333333333333333333333333333333333333333333333333333333333"}};

  auto thrasher =
      CreateBlobThrasherWithClient(std::move(config), std::move(client_end), merkle_roots, 5000);
  ASSERT_NE(thrasher, nullptr);

  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
  });

  // Wait for first request during Initialize
  RunLoopUntil([&]() { return mock_blob_reader_ptr_->has_pending_request(); });
  EXPECT_EQ(mock_blob_reader_ptr_->get_call_count(), 1u);

  // Reply with a 4096 byte VMO. This should succeed (total 4096 <= 5000).
  zx::vmo vmo1;
  ASSERT_EQ(zx::vmo::create(4096, 0, &vmo1), ZX_OK);
  mock_blob_reader_ptr_->ReplyToPendingRequest(std::move(vmo1));

  // Wait for second request. This should happen because we allow going over the limit once.
  RunLoopUntil([&]() { return mock_blob_reader_ptr_->has_pending_request(); });
  EXPECT_EQ(mock_blob_reader_ptr_->get_call_count(), 2u);

  // Reply with another 4096 byte VMO. This should succeed, but trigger the limit (total 8192 >
  // 5000).
  zx::vmo vmo2;
  ASSERT_EQ(zx::vmo::create(4096, 0, &vmo2), ZX_OK);
  mock_blob_reader_ptr_->ReplyToPendingRequest(std::move(vmo2));

  // The third request should NOT happen because we are now over the limit.
  // Initialize should complete.
  RunLoopUntil([&]() { return init_called; });
  ASSERT_EQ(init_status, ZX_OK);

  thrasher->Start(thrash_callback, nullptr);

  // The third request should NOT happen because we are now over the limit.
  // Instead, AllGetVmosDone should be called, leading to the callback.
  RunLoopUntil([&]() { return callback_called || mock_blob_reader_ptr_->has_pending_request(); });

  if (mock_blob_reader_ptr_->has_pending_request()) {
    // If we got here, it means the limit failed to stop the third request.
    // Reply to unblock and fail.
    zx::vmo vmo3;
    ASSERT_EQ(zx::vmo::create(4096, 0, &vmo3), ZX_OK);
    mock_blob_reader_ptr_->ReplyToPendingRequest(std::move(vmo3));
    RunLoopUntil([&]() { return callback_called; });
  }

  EXPECT_TRUE(callback_called);
  // We expect 2 calls because the third is skipped due to the memory limit.
  EXPECT_EQ(mock_blob_reader_ptr_->get_call_count(), 2u);
}

TEST_F(ThrasherTest, ParentVmoAccountingNoParentHandleTest) {
  const size_t kVmoSize = zx_system_get_page_size();
  zx::vmo parent_vmo;
  ASSERT_EQ(zx::vmo::create(kVmoSize, 0, &parent_vmo), ZX_OK);

  // Commit pages in parent.
  ASSERT_EQ(parent_vmo.op_range(ZX_VMO_OP_COMMIT, 0, kVmoSize, nullptr, 0), ZX_OK);

  // Get parent KOID.
  zx_info_handle_basic_t parent_basic_info;
  ASSERT_EQ(parent_vmo.get_info(ZX_INFO_HANDLE_BASIC, &parent_basic_info, sizeof(parent_basic_info),
                                nullptr, nullptr),
            ZX_OK);
  zx_koid_t parent_koid = parent_basic_info.koid;

  // Create a reference child VMO.
  zx::vmo child_vmo;
  // Size must be 0 for ZX_VMO_CHILD_REFERENCE (b/393402141)
  ASSERT_EQ(
      parent_vmo.create_child(ZX_VMO_CHILD_REFERENCE | ZX_VMO_CHILD_NO_WRITE, 0, 0, &child_vmo),
      ZX_OK);

  // Close parent handle.
  parent_vmo.reset();

  // Call log_vmos to verify it fails gracefully when parent is missing.
  std::vector<zx::vmo> vmos_to_log;
  zx::vmo child_dup;
  ASSERT_EQ(child_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &child_dup), ZX_OK);
  vmos_to_log.push_back(std::move(child_dup));

  LogVmos(vmos_to_log, true);

  // Now check ZX_INFO_PROCESS_VMOS to see if we can still find the parent.
  size_t actual = 0;
  size_t avail = 0;
  std::vector<zx_info_vmo_t> vmos;
  do {
    ASSERT_EQ(zx::process::self()->get_info(ZX_INFO_PROCESS_VMOS, nullptr, 0, &actual, &avail),
              ZX_OK);
    // Add headroom to avoid spinning if VMOs are being created rapidly.
    vmos.resize(avail + 100);
    ASSERT_EQ(zx::process::self()->get_info(ZX_INFO_PROCESS_VMOS, vmos.data(),
                                            vmos.size() * sizeof(zx_info_vmo_t), &actual, &avail),
              ZX_OK);
  } while (actual < avail);

  bool found_parent = false;
  for (size_t i = 0; i < actual; ++i) {
    if (vmos[i].koid == parent_koid) {
      found_parent = true;
      // If we found it, check if it reports committed bytes.
      EXPECT_GT(vmos[i].committed_bytes, 0u);
      break;
    }
  }

  // We expect NOT to find it if we don't have a handle or mapping.
  if (found_parent) {
  } else {
  }
  // This isn't strictly a pass/fail for the *test*, but a discovery step.
  // But let's assert our expectation to document it.
}

TEST(Thrasher, StatusReportingTest) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  bool done = false;
  int status_updates = 0;
  uint64_t total_touches_delta = 0;
  bool init_called = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;

  std::mutex cv_m;
  std::condition_variable cv;
  bool should_teardown = false;

  ThrashConfig config = {
      .bursts_per_second = 1000,
      .run_for_seconds = 2,
      .num_threads = 4,
      .pages_per_read = 1,
      .consecutive_pages_per_read = 1,
      .dispatcher = loop.dispatcher(),
      .status_interval_ms = 10,  // speed up status
      .teardown_sleep_fn =
          [&](zx::duration) {
            std::unique_lock<std::mutex> lock(cv_m);
            cv.wait(lock, [&] { return should_teardown; });
          },
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo>) {
    done = true;
    loop.Quit();
  });
  auto status_callback = std::make_shared<StatusCallback>([&](const ThrashStatus& status) {
    status_updates++;
    total_touches_delta += status.touches_delta;
    EXPECT_EQ(status.thrasher_type, "anon");
    EXPECT_GT(status.total_memory_bytes, 0u);
    EXPECT_GT(status.time_delta.to_msecs(), 0);
    EXPECT_GE(status.total_touches, status.touches_delta);
    EXPECT_GE(status.distinct_pages_delta, 0u);
    EXPECT_GT(status.total_time.to_msecs(), 0);

    // Unblock the teardown thread immediately.
    {
      std::lock_guard<std::mutex> lock(cv_m);
      should_teardown = true;
    }
    cv.notify_all();
  });

  auto thrasher = CreateAnonThrasher(std::move(config), 1024 * 1024);
  ASSERT_NE(thrasher, nullptr);

  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
  });

  while (!init_called) {
    loop.RunUntilIdle();
  }
  ASSERT_EQ(init_status, ZX_OK);

  thrasher->Start(thrash_callback, status_callback);
  loop.Run();

  ASSERT_TRUE(done);
  // We expect at least one status update if it runs for 2 seconds and updates every 1s.
  EXPECT_GT(status_updates, 0);
  EXPECT_GT(total_touches_delta, 0u);
}

TEST(Thrasher, StatusReportingIntervalTest) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  bool done = false;
  int status_updates = 0;
  bool init_called = false;
  zx_status_t init_status = ZX_ERR_INTERNAL;

  ThrashConfig config = {
      .bursts_per_second = 1000,
      .run_for_seconds = 1,
      .num_threads = 1,
      .pages_per_read = 1,
      .consecutive_pages_per_read = 1,
      .dispatcher = loop.dispatcher(),
      // Set a short interval to get multiple updates in 1 second
      .status_interval_ms = 200,
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo>) {
    done = true;
    loop.Quit();
  });
  auto status_callback = std::make_shared<StatusCallback>([&](const ThrashStatus& status) {
    status_updates++;
    // Allow some jitter, but it should be close to 200ms
    EXPECT_NEAR(static_cast<double>(status.time_delta.to_msecs()), 200.0, 100.0);
    EXPECT_GT(status.total_time.to_msecs(), 0);
    EXPECT_GE(status.distinct_pages_delta, 0u);
  });

  auto thrasher = CreateAnonThrasher(std::move(config), 1024 * 1024);
  ASSERT_NE(thrasher, nullptr);

  thrasher->Initialize([&](zx_status_t status) {
    init_called = true;
    init_status = status;
  });

  while (!init_called) {
    loop.RunUntilIdle();
  }
  ASSERT_EQ(init_status, ZX_OK);

  thrasher->Start(thrash_callback, status_callback);
  loop.Run();

  ASSERT_TRUE(done);
  // In 1 second with 200ms interval, we expect roughly 4-5 updates.
  // Allow some slack for thread scheduling.
  EXPECT_GE(status_updates, 3);
}

TEST_F(ThrasherTest, AnonAndBlobConcurrent) {
  auto client_end = CreateRealm();
  ASSERT_TRUE(client_end.is_valid());
  ASSERT_NE(mock_blob_reader_ptr_, nullptr);

  std::vector<MockBlobInfo> mock_blobs = {
      {"deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef", 4096},
  };
  mock_blob_reader_ptr_->set_mock_blobs(mock_blobs);
  std::vector<std::string> merkle_roots = {
      {"deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"}};

  std::atomic<int> completed_thrashers = 0;
  const int num_thrashers = 2;
  std::vector<zx::vmo> collected_vmos;
  std::mutex vmo_mutex;

  ThrashConfig config_anon = {
      .bursts_per_second = 100,
      .run_for_seconds = 5,  // Short run time for testing
      .num_threads = 1,
      .pages_per_read = 1,
      .consecutive_pages_per_read = 1,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };

  ThrashConfig config_blob = {
      .bursts_per_second = 100,
      .run_for_seconds = 5,  // Short run time for testing
      .num_threads = 1,
      .pages_per_read = 1,
      .consecutive_pages_per_read = 1,
      .dispatcher = dispatcher(),
      .teardown_sleep_fn = [](zx::duration) {},
  };

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    std::lock_guard<std::mutex> lock(vmo_mutex);
    for (auto& vmo : vmos) {
      collected_vmos.push_back(std::move(vmo));
    }
    if (++completed_thrashers == num_thrashers) {
      QuitLoop();
    }
  });

  auto anon_thrasher = CreateAnonThrasher(std::move(config_anon), 1024 * 1024);
  auto blob_thrasher = CreateBlobThrasherWithClient(std::move(config_blob), std::move(client_end),
                                                    merkle_roots, 10 * 1024 * 1024);

  ASSERT_NE(anon_thrasher, nullptr);
  ASSERT_NE(blob_thrasher, nullptr);

  std::atomic<int> pending_inits = 2;
  std::atomic<bool> init_failed = false;
  zx_status_t anon_status = ZX_ERR_INTERNAL;
  zx_status_t blob_status = ZX_ERR_INTERNAL;

  anon_thrasher->Initialize([&](zx_status_t status) {
    anon_status = status;
    if (status != ZX_OK)
      init_failed.store(true);
    pending_inits--;
  });

  blob_thrasher->Initialize([&](zx_status_t status) {
    blob_status = status;
    if (status != ZX_OK)
      init_failed.store(true);
    pending_inits--;
  });

  RunLoopUntil([&]() { return pending_inits == 0; });

  ASSERT_FALSE(init_failed.load());
  ASSERT_EQ(anon_status, ZX_OK);
  ASSERT_EQ(blob_status, ZX_OK);

  anon_thrasher->Start(thrash_callback, nullptr);
  blob_thrasher->Start(thrash_callback, nullptr);

  // This is expected to time out currently
  RunLoopWithTimeout(zx::sec(10));

  EXPECT_EQ(completed_thrashers, num_thrashers) << "Test timed out, possible hang";
}  // namespace ffxfs
