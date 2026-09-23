// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/wire_test_base.h>
#include <fuchsia/hardware/adb/cpp/fidl.h>
#include <fuchsia/sys2/cpp/fidl.h>
#include <fuchsia/sys2/cpp/fidl_test_base.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/syslog/cpp/macros.h>

#include <queue>

#include <gtest/gtest.h>

#include "fidl/fuchsia.io/cpp/wire_types.h"
#include "src/developer/adb/third_party/adb-file-sync/file_sync_service.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

namespace adb_file_sync {

const std::string kComponent = "component";
const std::string kTest = "test";

class FakeFile : public fidl::testing::WireTestBase<fuchsia_io::File> {
 public:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    FX_LOGS(ERROR) << "Not implemented " << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void BindServer(async_dispatcher_t* dispatcher, zx::channel chan) {
    binding_ref_ = std::make_unique<fidl::ServerBindingRef<fuchsia_io::File>>(
        fidl::BindServer(dispatcher, fidl::ServerEnd<fuchsia_io::File>(std::move(chan)), this));
  }

  // fuchsia_io::File methods
  void GetAttributes(fuchsia_io::wire::NodeGetAttributesRequest* request,
                     GetAttributesCompleter::Sync& completer) override {
    fidl::WireTableFrame<fuchsia_io::wire::ImmutableNodeAttributes> immutable_frame_;
    fidl::WireTableFrame<fuchsia_io::wire::MutableNodeAttributes> mutable_frame_;
    auto immutable_builder = fuchsia_io::wire::ImmutableNodeAttributes::ExternalBuilder(
        fidl::ObjectView<fidl::WireTableFrame<fuchsia_io::wire::ImmutableNodeAttributes>>::
            FromExternal(&immutable_frame_));

    auto mutable_builder = fuchsia_io::wire::MutableNodeAttributes::ExternalBuilder(
        fidl::ObjectView<fidl::WireTableFrame<fuchsia_io::wire::MutableNodeAttributes>>::
            FromExternal(&mutable_frame_));
    uint64_t expected_id = 1;
    uint64_t expected_content_size = 10;
    uint64_t expected_storage_size = 20;
    uint64_t expected_link_count = 0;
    uint64_t expected_creation_time = 3;
    uint64_t expected_modification_time = 5;

    mutable_builder.mode(1);
    immutable_builder.id(fidl::ObjectView<uint64_t>::FromExternal(&expected_id));
    immutable_builder.content_size(
        fidl::ObjectView<uint64_t>::FromExternal(&expected_content_size));
    immutable_builder.storage_size(
        fidl::ObjectView<uint64_t>::FromExternal(&expected_storage_size));
    immutable_builder.link_count(fidl::ObjectView<uint64_t>::FromExternal(&expected_link_count));
    mutable_builder.creation_time(
        fidl::ObjectView<uint64_t>::FromExternal(&expected_creation_time));
    mutable_builder.modification_time(
        fidl::ObjectView<uint64_t>::FromExternal(&expected_modification_time));
    completer.ReplySuccess(mutable_builder.Build(), immutable_builder.Build());
  }

  void Close(CloseCompleter::Sync& completer) override { completer.ReplySuccess(); }

  void Read(fuchsia_io::wire::ReadableReadRequest* request,
            ReadCompleter::Sync& completer) override {
    completer.ReplySuccess(fidl::VectorView<uint8_t>::FromExternal(data_));
    data_.clear();
  }

  void Write(fuchsia_io::wire::WritableWriteRequest* request,
             WriteCompleter::Sync& completer) override {
    std::copy(request->data.begin(), request->data.end(), std::back_inserter(data_));
    completer.ReplySuccess(request->data.size());
  }

  void set_data(std::vector<uint8_t> data) { data_ = std::move(data); }
  std::vector<uint8_t>& data() { return data_; }

 private:
  std::unique_ptr<fidl::ServerBindingRef<fuchsia_io::File>> binding_ref_;
  std::vector<uint8_t> data_;
};

class FakeDirectory : public fidl::testing::WireTestBase<fuchsia_io::Directory> {
 public:
  explicit FakeDirectory(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
    FX_LOGS(ERROR) << "Not implemented " << name;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  zx::channel BindServer() {
    auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    binding_ref_ = std::make_unique<fidl::ServerBindingRef<fuchsia_io::Directory>>(
        fidl::BindServer(dispatcher_, std::move(endpoints.server), this));
    return endpoints.client.TakeChannel();
  }

  void TearDown() {
    EXPECT_EQ(expect_read_dirents_.size(), 0U);
    EXPECT_EQ(expect_rewind_, 0U);
  }

  // fuchsia_io::Directory methods
  void GetAttributes(fuchsia_io::wire::NodeGetAttributesRequest* request,
                     GetAttributesCompleter::Sync& completer) override {
    auto ret = expect_get_attr_.front();
    expect_get_attr_.pop();
    completer.ReplySuccess(ret.mutable_attributes, ret.immutable_attributes);
  }
  void ExpectGetAttributes(fuchsia_io::wire::NodeAttributes2 attr) { expect_get_attr_.push(attr); }

  void Open(fuchsia_io::wire::OpenableOpenRequest* request,
            OpenCompleter::Sync& completer) override {
    file_.BindServer(dispatcher_, std::move(request->object));
  }
  FakeFile file_;  // Only allow one open file at a time for tests. Hardcoded file parameters.

  void ReadDirents(fuchsia_io::wire::DirectoryReadDirentsRequest* request,
                   ReadDirentsCompleter::Sync& completer) override {
    if (expect_read_dirents_.empty()) {
      completer.Reply(ZX_OK, {});
      return;
    }
    auto ret = std::move(expect_read_dirents_.front());
    expect_read_dirents_.pop();
    completer.Reply(ZX_OK, fidl::VectorView<uint8_t>::FromExternal(ret));
  }
  void ExpectReadDirents(std::vector<uint8_t> dirent) {
    expect_read_dirents_.push(std::move(dirent));
  }

  void Rewind(RewindCompleter::Sync& completer) override {
    ASSERT_GE(expect_rewind_, 1U);
    expect_rewind_--;
    completer.Reply(ZX_OK);
  }
  void ExpectRewind() { expect_rewind_++; }

 private:
  async_dispatcher_t* dispatcher_;
  std::unique_ptr<fidl::ServerBindingRef<fuchsia_io::Directory>> binding_ref_;

  std::queue<fuchsia_io::wire::NodeAttributes2> expect_get_attr_;
  std::queue<std::vector<uint8_t>> expect_read_dirents_;
  uint32_t expect_rewind_ = 0;
};

class LocalRealmQueryImpl : public fuchsia::sys2::testing::RealmQuery_TestBase,
                            public component_testing::LocalComponentImpl {
 public:
  explicit LocalRealmQueryImpl(async_dispatcher_t* dispatcher, FakeDirectory* directory)
      : dispatcher_(dispatcher),
        ns_directory_(directory),
        exposed_dir_(dispatcher),
        pkg_dir_(dispatcher) {}

  // component_testing::LocalComponentImpl methods
  void OnStart() override {
    ASSERT_EQ(outgoing()->AddPublicService(bindings_.GetHandler(this, dispatcher_),
                                           "fuchsia.sys2.RealmQuery.root"),
              ZX_OK);
  }

  // fuchsia_sys2::RealmQuery methods
  void NotImplemented_(const std::string& name) override {
    FX_LOGS(ERROR) << "Not implemented " << name;
  }

  void ConstructNamespace(::std::string moniker, ConstructNamespaceCallback callback) override {
    EXPECT_EQ(moniker, "./" + kComponent);

    auto ns_entries = std::vector<fuchsia::component::runner::ComponentNamespaceEntry>();
    ns_entries.emplace_back();
    ns_entries.back()
        .set_path("/" + kTest)
        .set_directory(fidl::InterfaceHandle<fuchsia::io::Directory>(ns_directory_->BindServer()));
    callback(fuchsia::sys2::RealmQuery_ConstructNamespace_Result::WithResponse(
        fuchsia::sys2::RealmQuery_ConstructNamespace_Response(std::move(ns_entries))));
  }

 private:
  async_dispatcher_t* dispatcher_;
  fidl::BindingSet<fuchsia::sys2::RealmQuery> bindings_;
  FakeDirectory* ns_directory_;
  FakeDirectory exposed_dir_;  // Not used
  FakeDirectory pkg_dir_;      // Not used
};

class LocalLifecycleController : public fuchsia::sys2::testing::LifecycleController_TestBase,
                                 public component_testing::LocalComponentImpl {
 public:
  explicit LocalLifecycleController(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  void NotImplemented_(const std::string& name) override {
    FX_LOGS(ERROR) << "Not implemented " << name;
  }

  void ResolveInstance(std::string moniker, ResolveInstanceCallback callback) override {
    moniker_ = moniker;
    EXPECT_EQ(moniker_, "./" + kComponent);
    callback(fuchsia::sys2::LifecycleController_ResolveInstance_Result::WithResponse(
        fuchsia::sys2::LifecycleController_ResolveInstance_Response(ZX_OK)));
  }

  // component_testing::LocalComponentImpl methods
  void OnStart() override {
    ASSERT_EQ(outgoing()->AddPublicService(bindings_.GetHandler(this, dispatcher_),
                                           "fuchsia.sys2.LifecycleController.root"),
              ZX_OK);
  }

  std::string moniker() { return moniker_; }

 private:
  // Lifecycle controller server bindings.
  fidl::BindingSet<fuchsia::sys2::LifecycleController> bindings_;
  // Moniker string passed in the latest request if any.
  std::string moniker_;
  async_dispatcher_t* dispatcher_;
};

class AdbFileSyncTest : public gtest::RealLoopFixture {
 public:
  AdbFileSyncTest() : directory_(dispatcher()) {}

  void Build(std::string default_moniker = "") {
    using namespace component_testing;
    auto builder = RealmBuilder::Create();
    builder.AddLocalChild("realm_query", [&]() {
      return std::make_unique<LocalRealmQueryImpl>(dispatcher(), &directory_);
    });
    builder.AddLocalChild("lifecycle_controller", [&]() {
      return std::make_unique<LocalLifecycleController>(dispatcher());
    });

    builder.AddChild("adb-file-sync", "#meta/adb-file-sync.cm");

    builder.AddRoute(Route{.capabilities = {Protocol{"fuchsia.sys2.RealmQuery.root"}},
                           .source = ChildRef{"realm_query"},
                           .targets = {ChildRef{"adb-file-sync"}}});
    builder.AddRoute(Route{.capabilities = {Protocol{"fuchsia.sys2.LifecycleController.root"}},
                           .source = ChildRef{"lifecycle_controller"},
                           .targets = {ChildRef{"adb-file-sync"}}});
    builder.AddRoute(Route{.capabilities = {Protocol{fuchsia::hardware::adb::Provider::Name_}},
                           .source = ChildRef{"adb-file-sync"},
                           .targets = {ParentRef()}});
    builder.InitMutableConfigToEmpty("adb-file-sync");
    builder.SetConfigValue("adb-file-sync", "filesync_moniker", std::move(default_moniker));
    realm_ = builder.Build(dispatcher());

    ASSERT_EQ(realm_->component().Connect<fuchsia::hardware::adb::Provider>(
                  file_sync_.NewRequest(dispatcher())),
              ZX_OK);

    zx::socket server, client;
    ASSERT_EQ(zx::socket::create(ZX_SOCKET_STREAM, &server, &client), ZX_OK);
    fuchsia::hardware::adb::Provider_ConnectToService_Result result;
    file_sync_.set_error_handler(
        [](zx_status_t status) { FX_LOGS(INFO) << "File Sync could not connect " << status; });
    file_sync_->ConnectToService(std::move(client), "", [&](auto result) {
      ASSERT_FALSE(result.is_err());
      QuitLoop();
    });
    RunLoop();

    adb_ = std::move(server);
  }

  void TearDown() override {
    bool complete = false;
    realm_->Teardown([&](fit::result<fuchsia::component::Error> result) { complete = true; });
    RunLoopUntil([&]() { return complete; });
    directory_.TearDown();
  }

  void ExpectSendFail() {
    size_t actual;
    syncmsg msg;
    RunLoopUntil([&]() { return adb_.read(0, &msg.data, sizeof(msg.data), &actual) == ZX_OK; });
    EXPECT_EQ(actual, sizeof(msg.data));
    EXPECT_EQ(static_cast<int32_t>(msg.data.id), ID_FAIL);

    auto buffer = std::make_unique<uint8_t[]>(msg.data.size);
    RunLoopUntil([&]() { return adb_.read(0, buffer.get(), msg.data.size, &actual) == ZX_OK; });
    EXPECT_EQ(actual, msg.data.size);
  }

 private:
  std::optional<component_testing::RealmRoot> realm_;
  fidl::InterfacePtr<fuchsia::hardware::adb::Provider> file_sync_;

 protected:
  zx::socket adb_;
  FakeDirectory directory_;
};

TEST_F(AdbFileSyncTest, BadPathLengthConnectTest) {
  Build();
  SyncRequest request{.id = ID_LIST, .path_length = 1025};
  size_t actual;
  EXPECT_EQ(adb_.write(0, &request, sizeof(request), &actual), ZX_OK);
  EXPECT_EQ(actual, sizeof(request));
  ExpectSendFail();
}

TEST_F(AdbFileSyncTest, BadIDConnectTest) {
  Build();
  std::string filename = "filename";

  SyncRequest request{.id = MKID('B', 'A', 'D', 'D'),
                      .path_length = static_cast<uint32_t>(filename.size())};
  size_t actual;
  EXPECT_EQ(adb_.write(0, &request, sizeof(request), &actual), ZX_OK);
  EXPECT_EQ(actual, sizeof(request));

  EXPECT_EQ(adb_.write(0, filename.c_str(), filename.size(), &actual), ZX_OK);
  EXPECT_EQ(actual, filename.size());
  ExpectSendFail();
}

TEST_F(AdbFileSyncTest, HandleListTest) {
  Build();
  directory_.ExpectRewind();
  directory_.ExpectReadDirents({0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 'a', 'b'});
  std::string path = "./" + kComponent + "::/" + kTest;
  SyncRequest request{.id = ID_LIST, .path_length = static_cast<uint32_t>(path.length())};
  size_t actual;
  EXPECT_EQ(adb_.write(0, &request, sizeof(request), &actual), ZX_OK);
  EXPECT_EQ(actual, sizeof(request));

  EXPECT_EQ(adb_.write(0, path.c_str(), path.size(), &actual), ZX_OK);
  EXPECT_EQ(actual, path.size());

  // Read dirent
  syncmsg msg;
  RunLoopUntil([&]() { return adb_.read(0, &(msg.dent), sizeof(msg.dent), &actual) == ZX_OK; });
  EXPECT_EQ(actual, sizeof(msg.dent));
  EXPECT_EQ(static_cast<int32_t>(msg.dent.id), ID_DENT);
  EXPECT_EQ(msg.dent.mode, 1U);
  EXPECT_EQ(msg.dent.namelen, 2U);
  EXPECT_EQ(msg.dent.size, 20U);
  EXPECT_EQ(msg.dent.time, 5U);

  // Read name
  char name[2];
  RunLoopUntil([&]() { return adb_.read(0, name, sizeof(name), &actual) == ZX_OK; });
  EXPECT_EQ(actual, sizeof(name));
  EXPECT_EQ(name[0], 'a');
  EXPECT_EQ(name[1], 'b');

  // Read done
  RunLoopUntil([&]() { return adb_.read(0, &(msg.dent), sizeof(msg.dent), &actual) == ZX_OK; });
  EXPECT_EQ(actual, sizeof(msg.dent));
  EXPECT_EQ(static_cast<int32_t>(msg.dent.id), ID_DONE);
  EXPECT_EQ(msg.dent.mode, 0U);
  EXPECT_EQ(msg.dent.namelen, 0U);
  EXPECT_EQ(msg.dent.size, 0U);
  EXPECT_EQ(msg.dent.time, 0U);
}

TEST_F(AdbFileSyncTest, HandleListTestWithDefault) {
  Build("/" + kComponent);
  directory_.ExpectRewind();
  directory_.ExpectReadDirents({0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 'a', 'b'});
  std::string path = "/" + kTest;
  SyncRequest request{.id = ID_LIST, .path_length = static_cast<uint32_t>(path.length())};
  size_t actual;
  EXPECT_EQ(adb_.write(0, &request, sizeof(request), &actual), ZX_OK);
  EXPECT_EQ(actual, sizeof(request));

  EXPECT_EQ(adb_.write(0, path.c_str(), path.size(), &actual), ZX_OK);
  EXPECT_EQ(actual, path.size());

  // Read dirent
  RunLoopUntil([&] {
    return adb_.wait_one(ZX_SOCKET_READABLE | ZX_SOCKET_PEER_CLOSED,
                         zx::deadline_after(zx::msec(10)), nullptr) == ZX_OK;
  });
  syncmsg msg;
  EXPECT_EQ(adb_.read(0, &(msg.dent), sizeof(msg.dent), &actual), ZX_OK);
  EXPECT_EQ(actual, sizeof(msg.dent));
  EXPECT_EQ(static_cast<int32_t>(msg.dent.id), ID_DENT);
  EXPECT_EQ(msg.dent.mode, 1U);
  EXPECT_EQ(msg.dent.namelen, 2U);
  EXPECT_EQ(msg.dent.size, 20U);
  EXPECT_EQ(msg.dent.time, 5U);

  // Read name
  RunLoopUntil([&] {
    return adb_.wait_one(ZX_SOCKET_READABLE | ZX_SOCKET_PEER_CLOSED,
                         zx::deadline_after(zx::msec(10)), nullptr) == ZX_OK;
  });
  char name[2];
  EXPECT_EQ(adb_.read(0, name, sizeof(name), &actual), ZX_OK);
  EXPECT_EQ(actual, sizeof(name));
  EXPECT_EQ(name[0], 'a');
  EXPECT_EQ(name[1], 'b');

  // Read done
  RunLoopUntil([&] {
    return adb_.wait_one(ZX_SOCKET_READABLE | ZX_SOCKET_PEER_CLOSED,
                         zx::deadline_after(zx::msec(10)), nullptr) == ZX_OK;
  });
  EXPECT_EQ(adb_.read(0, &(msg.dent), sizeof(msg.dent), &actual), ZX_OK);
  EXPECT_EQ(actual, sizeof(msg.dent));
  EXPECT_EQ(static_cast<int32_t>(msg.dent.id), ID_DONE);
  EXPECT_EQ(msg.dent.mode, 0U);
  EXPECT_EQ(msg.dent.namelen, 0U);
  EXPECT_EQ(msg.dent.size, 0U);
  EXPECT_EQ(msg.dent.time, 0U);
}

TEST_F(AdbFileSyncTest, HandleStatTest) {
  Build();
  fidl::WireTableFrame<fuchsia_io::wire::ImmutableNodeAttributes> immutable_frame_;
  fidl::WireTableFrame<fuchsia_io::wire::MutableNodeAttributes> mutable_frame_;

  auto immutable_builder = fuchsia_io::wire::ImmutableNodeAttributes::ExternalBuilder(
      fidl::ObjectView<fidl::WireTableFrame<fuchsia_io::wire::ImmutableNodeAttributes>>::
          FromExternal(&immutable_frame_));

  auto mutable_builder = fuchsia_io::wire::MutableNodeAttributes::ExternalBuilder(
      fidl::ObjectView<fidl::WireTableFrame<fuchsia_io::wire::MutableNodeAttributes>>::FromExternal(
          &mutable_frame_));
  uint64_t expected_storage_size = 15;
  uint64_t expected_modification_time = 1234;

  mutable_builder.mode(5);
  immutable_builder.storage_size(fidl::ObjectView<uint64_t>::FromExternal(&expected_storage_size));
  mutable_builder.modification_time(
      fidl::ObjectView<uint64_t>::FromExternal(&expected_modification_time));

  directory_.ExpectGetAttributes(fuchsia_io::wire::NodeAttributes2{
      .mutable_attributes = mutable_builder.Build(),
      .immutable_attributes = immutable_builder.Build(),
  });

  std::string path = "./" + kComponent + "::/" + kTest;
  SyncRequest request{.id = ID_LSTAT_V1, .path_length = static_cast<uint32_t>(path.length())};
  size_t actual;
  EXPECT_EQ(adb_.write(0, &request, sizeof(request), &actual), ZX_OK);
  EXPECT_EQ(actual, sizeof(request));

  EXPECT_EQ(adb_.write(0, path.c_str(), path.size(), &actual), ZX_OK);
  EXPECT_EQ(actual, path.size());

  // Read stat
  syncmsg msg;
  RunLoopUntil([&]() { return adb_.read(0, &msg.stat_v1, sizeof(msg.stat_v1), &actual) == ZX_OK; });
  EXPECT_EQ(actual, sizeof(msg.stat_v1));
  EXPECT_EQ(static_cast<int32_t>(msg.stat_v1.id), ID_LSTAT_V1);
  EXPECT_EQ(msg.stat_v1.mode, 5U);
  EXPECT_EQ(msg.stat_v1.size, 15U);
  EXPECT_EQ(msg.stat_v1.time, 1234U);
}

TEST_F(AdbFileSyncTest, HandleSendTest) {
  Build();
  std::string path = "./" + kComponent + "::/" + kTest + "/tmp.txt,0755";
  SyncRequest request{.id = ID_SEND, .path_length = static_cast<uint32_t>(path.length())};
  size_t actual;
  EXPECT_EQ(adb_.write(0, &request, sizeof(request), &actual), ZX_OK);
  EXPECT_EQ(actual, sizeof(request));

  EXPECT_EQ(adb_.write(0, path.c_str(), path.size(), &actual), ZX_OK);
  EXPECT_EQ(actual, path.size());

  // Send data
  uint8_t buffer[] = {1, 2, 3, 4};
  syncmsg msg;
  msg.data.id = ID_DATA;
  msg.data.size = sizeof(buffer);
  EXPECT_EQ(adb_.write(0, &(msg.data), sizeof(msg.data), &actual), ZX_OK);
  EXPECT_EQ(actual, sizeof(msg.data));
  EXPECT_EQ(adb_.write(0, buffer, sizeof(buffer), &actual), ZX_OK);
  EXPECT_EQ(actual, sizeof(buffer));

  EXPECT_EQ(adb_.write(0, &(msg.data), sizeof(msg.data), &actual), ZX_OK);
  EXPECT_EQ(actual, sizeof(msg.data));
  EXPECT_EQ(adb_.write(0, buffer, sizeof(buffer), &actual), ZX_OK);
  EXPECT_EQ(actual, sizeof(buffer));

  // Send done
  msg.data.id = ID_DONE;
  msg.data.size = 0;
  EXPECT_EQ(adb_.write(0, &(msg.data), sizeof(msg.data), &actual), ZX_OK);
  EXPECT_EQ(actual, sizeof(msg.data));

  // Read done
  RunLoopUntil([&]() { return adb_.read(0, &(msg.data), sizeof(msg.data), &actual) == ZX_OK; });
  EXPECT_EQ(actual, sizeof(msg.data));
  EXPECT_EQ(static_cast<int32_t>(msg.data.id), ID_OKAY);
  EXPECT_EQ(msg.data.size, 0U);

  uint8_t expected_data[] = {1, 2, 3, 4, 1, 2, 3, 4};
  EXPECT_EQ(directory_.file_.data().size(), sizeof(expected_data));
  for (size_t i = 0; i < sizeof(expected_data); i++) {
    EXPECT_EQ(directory_.file_.data()[i], expected_data[i]);
  }
}

TEST_F(AdbFileSyncTest, HandleReceiveTest) {
  Build();
  directory_.file_.set_data(std::vector<uint8_t>{4, 3, 2, 1});

  std::string path = "./" + kComponent + "::/" + kTest + "/tmp.txt";
  SyncRequest request{.id = ID_RECV, .path_length = static_cast<uint32_t>(path.length())};
  size_t actual;
  EXPECT_EQ(adb_.write(0, &request, sizeof(request), &actual), ZX_OK);
  EXPECT_EQ(actual, sizeof(request));

  EXPECT_EQ(adb_.write(0, path.c_str(), path.size(), &actual), ZX_OK);
  EXPECT_EQ(actual, path.size());

  // Read data
  syncmsg msg;
  RunLoopUntil([&]() { return adb_.read(0, &(msg.data), sizeof(msg.data), &actual) == ZX_OK; });
  EXPECT_EQ(actual, sizeof(msg.data));
  EXPECT_EQ(static_cast<int32_t>(msg.data.id), ID_DATA);
  EXPECT_EQ(msg.data.size, 4U);

  auto buffer = std::make_unique<uint8_t[]>(msg.data.size);
  RunLoopUntil([&]() { return adb_.read(0, buffer.get(), msg.data.size, &actual) == ZX_OK; });
  EXPECT_EQ(actual, msg.data.size);

  uint8_t expected_data[] = {4, 3, 2, 1};
  EXPECT_EQ(msg.data.size, sizeof(expected_data));
  for (size_t i = 0; i < msg.data.size; i++) {
    EXPECT_EQ(buffer.get()[i], expected_data[i]);
  }

  // Read done
  RunLoopUntil([&]() { return adb_.read(0, &(msg.data), sizeof(msg.data), &actual) == ZX_OK; });
  EXPECT_EQ(actual, sizeof(msg.data));
  EXPECT_EQ(static_cast<int32_t>(msg.data.id), ID_DONE);
  EXPECT_EQ(msg.data.size, 0U);
}

}  // namespace adb_file_sync
