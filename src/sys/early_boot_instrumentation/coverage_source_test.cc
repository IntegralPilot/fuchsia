// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/early_boot_instrumentation/coverage_source.h"

#include <fcntl.h>
#include <fidl/fuchsia.boot/cpp/markers.h>
#include <fidl/fuchsia.boot/cpp/wire.h>
#include <fidl/fuchsia.debugdata/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/namespace.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/fidl/cpp/wire/message_storage.h>
#include <lib/fidl/cpp/wire/string_view.h>
#include <lib/stdcompat/array.h>
#include <lib/stdcompat/span.h>
#include <lib/zbi-format/internal/debugdata.h>
#include <lib/zbitl/item.h>
#include <lib/zx/channel.h>
#include <lib/zx/event.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/vmo.h>
#include <stdio.h>
#include <sys/stat.h>
#include <zircon/errors.h>
#include <zircon/syscalls/object.h>

#include <cstdint>
#include <memory>
#include <string_view>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <sdk/lib/vfs/cpp/pseudo_dir.h>
#include <sdk/lib/vfs/cpp/vmo_file.h>

namespace early_boot_instrumentation {
namespace {

constexpr auto kServeFlags = fuchsia_io::kPermReadable;

// Serve vmos from arbitrary paths in the local namespace.
class FakeBootItemsFixture : public testing::Test {
 public:
  // Root path from where to serve the hierarchy.
  void Serve(const std::string& path) {
    auto [dir_client, dir_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    fdio_ns_t* root_ns = nullptr;
    path_ = path;
    ASSERT_EQ(fdio_ns_get_installed(&root_ns), ZX_OK);
    ASSERT_EQ(fdio_ns_bind(root_ns, path.c_str(), dir_client.TakeChannel().release()), ZX_OK);
    ASSERT_EQ(debugdata_dir_.Serve(kServeFlags, std::move(dir_server), loop_.dispatcher()), ZX_OK);
    loop_.StartThread("kernel_data_dir");
  }

  void BindFile(std::string_view path) { BindHierarchy(debugdata_dir_, path); }

  void TearDown() override {
    // Best effort.
    fdio_ns_t* root_ns = nullptr;
    ASSERT_EQ(fdio_ns_get_installed(&root_ns), ZX_OK);
    fdio_ns_unbind(root_ns, path_.c_str());
    loop_.Shutdown();
  }

 private:
  // directory components end with '/' and all paths are relative (no leading /).
  void BindHierarchy(vfs::PseudoDir& root, std::string_view path) {
    auto curr = path.find("/");
    // path is a file to be bound.
    if (curr == std::string_view::npos) {
      zx::vmo path_vmo;
      ASSERT_EQ(zx::vmo::create(4096, 0, &path_vmo), 0);
      auto file = std::make_unique<vfs::VmoFile>(std::move(path_vmo), 4096);
      ASSERT_EQ(root.AddEntry(std::string(path), std::move(file)), ZX_OK);
      return;
    }

    // curr is a directory, and we continue to bind directories and strip components.
    std::string dir_name(path.substr(0, curr));
    path = path.substr(curr + 1, path.length());
    // see if dir exists.
    vfs::Node* existing_entry = nullptr;
    if (root.Lookup(dir_name, &existing_entry) == ZX_ERR_NOT_FOUND) {
      std::unique_ptr<vfs::PseudoDir> new_dir = std::make_unique<vfs::PseudoDir>();
      existing_entry = new_dir.get();
      root.AddEntry(dir_name, std::move(new_dir));
    }

    ASSERT_NE(existing_entry, nullptr);
    BindHierarchy(*reinterpret_cast<vfs::PseudoDir*>(existing_entry), path);
  }

  async::Loop loop_ = async::Loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  vfs::PseudoDir debugdata_dir_;

  std::string path_;
};

using ExposeDebugdataTest = FakeBootItemsFixture;

TEST_F(ExposeDebugdataTest, SingleSinkStatic) {
  BindFile("random-sink/s/my-sink-data.my-data");
  ASSERT_NO_FATAL_FAILURE(Serve("/boot/kernel/i"));

  fbl::unique_fd debugdata_dir(open("/boot/kernel/i", O_RDONLY));
  ASSERT_TRUE(debugdata_dir) << strerror(errno);

  SinkDirMap sink_map;

  ASSERT_TRUE(ExposeBootDebugdata(debugdata_dir, sink_map).is_ok());
  vfs::PseudoDir* lookup = nullptr;
  ASSERT_EQ(sink_map["random-sink"]->Lookup("static", reinterpret_cast<vfs::Node**>(&lookup)),
            ZX_OK);
  vfs::PseudoDir& out_dir = *lookup;
  ASSERT_FALSE(out_dir.IsEmpty());

  vfs::Node* node = nullptr;
  ASSERT_EQ(out_dir.Lookup("my-sink-data.my-data", &node), ZX_OK);
  ASSERT_NE(node, nullptr);
}

TEST_F(ExposeDebugdataTest, SingleSinkDynamic) {
  BindFile("random-sink/d/my-sink-data.my-data");
  ASSERT_NO_FATAL_FAILURE(Serve("/boot/kernel/i"));

  fbl::unique_fd debugdata_dir(open("/boot/kernel/i", O_RDONLY));
  ASSERT_TRUE(debugdata_dir) << strerror(errno);

  SinkDirMap sink_map;

  ASSERT_TRUE(ExposeBootDebugdata(debugdata_dir, sink_map).is_ok());
  vfs::PseudoDir* lookup = nullptr;
  ASSERT_EQ(sink_map["random-sink"]->Lookup("dynamic", reinterpret_cast<vfs::Node**>(&lookup)),
            ZX_OK);
  vfs::PseudoDir& out_dir = *lookup;
  ASSERT_FALSE(out_dir.IsEmpty());

  vfs::Node* node = nullptr;
  ASSERT_EQ(out_dir.Lookup("my-sink-data.my-data", &node), ZX_OK);
  ASSERT_NE(node, nullptr);
}

TEST_F(ExposeDebugdataTest, MultipleSinks) {
  BindFile("random-sink/s/my-sink-data.my-data");
  BindFile("random-sink/d/my-dsink-data.my-data");
  BindFile("other-random-sink/s/my-other-sink-data.my-data");
  BindFile("other-random-sink/d/my-other-dsink-data.my-data");
  ASSERT_NO_FATAL_FAILURE(Serve("/boot/kernel/i"));

  fbl::unique_fd debugdata_dir(open("/boot/kernel/i", O_RDONLY));
  ASSERT_TRUE(debugdata_dir) << strerror(errno);

  SinkDirMap sink_map;

  ASSERT_TRUE(ExposeBootDebugdata(debugdata_dir, sink_map).is_ok());

  std::vector<std::tuple<std::string, std::string, std::string>> lookup_entries = {
      {"random-sink", "static", "my-sink-data.my-data"},
      {"random-sink", "dynamic", "my-dsink-data.my-data"},
      {"other-random-sink", "static", "my-other-sink-data.my-data"},
      {"other-random-sink", "dynamic", "my-other-dsink-data.my-data"},
  };

  for (const auto& [sink, data_dir, file_name] : lookup_entries) {
    vfs::PseudoDir* lookup = nullptr;
    ASSERT_EQ(sink_map[sink]->Lookup(data_dir, reinterpret_cast<vfs::Node**>(&lookup)), ZX_OK);
    vfs::PseudoDir& out_dir = *lookup;
    ASSERT_FALSE(out_dir.IsEmpty());

    vfs::Node* node = nullptr;
    ASSERT_EQ(out_dir.Lookup(file_name, &node), ZX_OK);
    ASSERT_NE(node, nullptr);
  }
}

TEST_F(ExposeDebugdataTest, MultipleSinksAndLogFile) {
  BindFile("logs/foo-logs");
  BindFile("random-sink/s/my-sink-data.my-data");
  BindFile("random-sink/d/my-dsink-data.my-data");
  BindFile("other-random-sink/s/my-other-sink-data.my-data");
  BindFile("other-random-sink/d/my-other-dsink-data.my-data");
  ASSERT_NO_FATAL_FAILURE(Serve("/boot/kernel/i"));

  fbl::unique_fd debugdata_dir(open("/boot/kernel/i", O_RDONLY));
  ASSERT_TRUE(debugdata_dir) << strerror(errno);

  SinkDirMap sink_map;

  ASSERT_TRUE(ExposeBootDebugdata(debugdata_dir, sink_map).is_ok());

  std::vector<std::tuple<std::string, std::string, std::string>> lookup_entries = {
      {"random-sink", "static", "my-sink-data.my-data"},
      {"random-sink", "dynamic", "my-dsink-data.my-data"},
      {"other-random-sink", "static", "my-other-sink-data.my-data"},
      {"other-random-sink", "dynamic", "my-other-dsink-data.my-data"},
  };

  EXPECT_EQ(sink_map.find("logs"), sink_map.end());
  EXPECT_EQ(sink_map.size(), 2u);

  for (const auto& [sink, data_dir, file_name] : lookup_entries) {
    vfs::PseudoDir* lookup = nullptr;
    ASSERT_EQ(sink_map[sink]->Lookup(data_dir, reinterpret_cast<vfs::Node**>(&lookup)), ZX_OK);
    vfs::PseudoDir& out_dir = *lookup;
    ASSERT_FALSE(out_dir.IsEmpty());

    vfs::Node* node = nullptr;
    ASSERT_EQ(out_dir.Lookup(file_name, &node), ZX_OK);
    ASSERT_NE(node, nullptr);
  }
}

using ExposeLogsTest = FakeBootItemsFixture;

TEST_F(ExposeLogsTest, MultipleSinksAndLogFile) {
  BindFile("logs/foo-logs");
  BindFile("logs/foo-logs2");
  BindFile("logs/foo-logs3");
  BindFile("random-sink/s/my-sink-data.my-data");
  BindFile("random-sink/d/my-dsink-data.my-data");
  BindFile("other-random-sink/s/my-other-sink-data.my-data");
  BindFile("other-random-sink/d/my-other-dsink-data.my-data");
  ASSERT_NO_FATAL_FAILURE(Serve("/boot/kernel/i"));

  auto out_log_dir = std::make_unique<vfs::PseudoDir>();
  fbl::unique_fd logs_dir(open("/boot/kernel/i/logs", O_RDONLY));
  ASSERT_TRUE(logs_dir) << strerror(errno);

  ASSERT_TRUE(ExposeLogs(logs_dir, *out_log_dir).is_ok());

  auto lookup = {"foo-logs", "foo-logs2", "foo-logs3"};
  for (auto log : lookup) {
    vfs::Node* node = nullptr;
    ASSERT_EQ(out_log_dir->Lookup(log, &node), ZX_OK);
    ASSERT_NE(node, nullptr);
  }
}

struct PublishRequest {
  std::string sink;
  bool peer_closed;
};

constexpr std::string_view kData = "12345670123";
constexpr size_t kDataOffset = 0xAD;

zx::result<zx::vmo> MakeTestVmo(uint32_t data_offset) {
  zx::vmo vmo;
  if (auto status = zx::vmo::create(4096, 0, &vmo); status != ZX_OK) {
    return zx::error(status);
  }
  if (auto status = vmo.write(kData.data(), kDataOffset + data_offset, kDataOffset);
      status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(vmo));
}

void ValidatePublishedRequests(uint32_t svc_index, cpp20::span<PublishRequest> requests,
                               SinkDirMap& sink_map) {
  for (uint32_t i = 0; i < requests.size(); ++i) {
    std::string path(requests[i].peer_closed ? kStaticDir : kDynamicDir);
    std::string name = std::to_string(svc_index) + "-" + std::to_string(i);
    if (requests[i].sink == kLlvmSink) {
      name += "." + std::string(kLlvmSinkExtension);
    }

    auto it = sink_map.find(requests[i].sink);
    ASSERT_NE(it, sink_map.end());
    auto& sink_root = *it->second;

    vfs::Node* lookup_node = nullptr;
    ASSERT_EQ(sink_root.Lookup(path, &lookup_node), ZX_OK);

    auto* typed_dir = reinterpret_cast<vfs::PseudoDir*>(lookup_node);
    ASSERT_EQ(typed_dir->Lookup(name, &lookup_node), ZX_OK) << name;

    auto* vmo_file = reinterpret_cast<vfs::VmoFile*>(lookup_node);
    std::vector<uint8_t> actual_data(kData.size());
    ASSERT_EQ(vmo_file->vmo()->read(actual_data.data(), kDataOffset + i, kData.size()), ZX_OK);

    EXPECT_TRUE(memcmp(kData.data(), actual_data.data(), kData.size()) == 0);
  }
}

void ValidatePublishedRequests(uint32_t svc_index, PublishRequest& request, SinkDirMap& sink_map) {
  ValidatePublishedRequests(svc_index, {&request, 1}, sink_map);
}

class ExtractDebugDataTest : public ::testing::Test {
 public:
  void SetUp() final {
    zx::result client_end = fidl::CreateEndpoints(&svc_stash_read_);
    ASSERT_TRUE(client_end.is_ok()) << client_end.status_string();
    svc_stash_.Bind(std::move(client_end.value()));
  }

  void StashSvcWithPublishedData(const PublishRequest& publish_info, zx::eventpair& out_token) {
    StashSvcWithPublishedData({&publish_info, 1}, {&out_token, 1});
  }

  // Same as above, but published multiple pairs of |<sink, vmo>| represented by sinks[i],
  // vmos[i]. |out| is the write end of the handle.
  void StashSvcWithPublishedData(cpp20::span<const PublishRequest> publish_info,
                                 cpp20::span<zx::eventpair> out_tokens) {
    ASSERT_EQ(publish_info.size(), out_tokens.size());

    auto [client_end, server_end] = fidl::Endpoints<fuchsia_io::Directory>::Create();

    const fidl::OneWayStatus status = svc_stash_->Store(std::move(server_end));
    ASSERT_TRUE(status.ok()) << status.FormatDescription();

    for (uint32_t i = 0; i < publish_info.size(); ++i) {
      zx::result vmo_or = MakeTestVmo(i);
      ASSERT_TRUE(vmo_or.is_ok()) << vmo_or.status_string();
      if (publish_info[i].sink == kLlvmSink) {
        vmo_or.value().set_property(ZX_PROP_NAME, kLlvmSinkExtension.data(),
                                    kLlvmSinkExtension.size());
      }
      PublishOne(client_end, publish_info[i].sink, std::move(vmo_or).value(), out_tokens[i]);
      if (publish_info[i].peer_closed) {
        out_tokens[i].reset();
      }
    }
  }

  auto&& take_stash_read() { return std::move(svc_stash_read_); }

 private:
  static void PublishOne(const fidl::ClientEnd<fuchsia_io::Directory>& directory,
                         std::string_view sink_name, zx::vmo vmo, zx::eventpair& out_token) {
    zx::eventpair token1, token2;
    ASSERT_EQ(zx::eventpair::create(0, &token1, &token2), ZX_OK);

    zx::result client_end = component::ConnectAt<fuchsia_debugdata::Publisher>(directory);
    ASSERT_TRUE(client_end.is_ok()) << client_end.status_string();

    fidl::WireSyncClient client(std::move(client_end.value()));
    const fidl::OneWayStatus result = client->Publish(fidl::StringView::FromExternal(sink_name),
                                                      std::move(vmo), std::move(token1));
    ASSERT_TRUE(result.ok()) << result.FormatDescription();

    out_token = std::move(token2);
  }

  fidl::ServerEnd<fuchsia_boot::SvcStash> svc_stash_read_;
  fidl::WireSyncClient<fuchsia_boot::SvcStash> svc_stash_;
};

TEST_F(ExtractDebugDataTest, NoRequestsIsEmpty) {
  auto svc_stash = take_stash_read();
  auto sink_map = ExtractDebugData(std::move(svc_stash));
  ASSERT_TRUE(sink_map.empty());
}

TEST_F(ExtractDebugDataTest, SingleStashedSvcWithSingleOutstandingPublishRequest) {
  auto svc_stash = take_stash_read();
  PublishRequest req = {"my-custom-sink", true};
  zx::eventpair token;

  ASSERT_NO_FATAL_FAILURE(StashSvcWithPublishedData(req, token));
  auto sink_map = ExtractDebugData(std::move(svc_stash));
  ASSERT_FALSE(sink_map.empty());
  ValidatePublishedRequests(0u, req, sink_map);
}

TEST_F(ExtractDebugDataTest, LlvmSinkHaveProfrawExtension) {
  auto svc_stash = take_stash_read();
  auto reqs = cpp20::to_array<PublishRequest>(
      {{std::string(kLlvmSink), true}, {std::string(kLlvmSink), false}});
  std::vector<zx::eventpair> tokens;
  tokens.resize(reqs.size());

  ASSERT_NO_FATAL_FAILURE(StashSvcWithPublishedData(reqs, tokens));

  auto sink_map = ExtractDebugData(std::move(svc_stash));
  ASSERT_FALSE(sink_map.empty());

  ValidatePublishedRequests(0u, reqs, sink_map);
}

TEST_F(ExtractDebugDataTest, SingleStashedSvcWithMultipleOutstandingPublishRequest) {
  auto svc_stash = take_stash_read();
  auto reqs = cpp20::to_array<PublishRequest>(
      {{"my-custom-sink", true}, {"another-sink", true}, {"my-custom-sink", false}});
  std::vector<zx::eventpair> tokens;
  tokens.resize(reqs.size());

  ASSERT_NO_FATAL_FAILURE(StashSvcWithPublishedData(reqs, tokens));

  auto sink_map = ExtractDebugData(std::move(svc_stash));
  ASSERT_FALSE(sink_map.empty());

  ValidatePublishedRequests(0u, reqs, sink_map);
}

TEST_F(ExtractDebugDataTest, MultipleStashedSvcWithSingleOutstandingPublishRequest) {
  auto svc_stash = take_stash_read();
  auto reqs = cpp20::to_array<PublishRequest>(
      {{"my-custom-sink", true}, {"another-sink", true}, {"my-custom-sink", false}});
  std::vector<zx::eventpair> tokens;
  tokens.resize(reqs.size());

  for (uint32_t i = 0; i < reqs.size(); ++i) {
    ASSERT_NO_FATAL_FAILURE(StashSvcWithPublishedData(reqs[i], tokens[i]));
  }

  auto sink_map = ExtractDebugData(std::move(svc_stash));
  ASSERT_FALSE(sink_map.empty());

  for (uint32_t i = 0; i < reqs.size(); ++i) {
    ValidatePublishedRequests(i, reqs[i], sink_map);
  }
}

// We need to fake a fuchsia.boot.Items FIDL serve.
class FakeFuchsiaBootItemServer : public fidl::WireServer<fuchsia_boot::Items> {
 public:
  explicit FakeFuchsiaBootItemServer(async_dispatcher_t* dispatcher) {
    zx::result server_end = fidl::CreateEndpoints(&svc_channel_);
    ZX_ASSERT(server_end.is_ok());
    fidl::BindServer(dispatcher, std::move(server_end).value(), this);
  }

  void AddDebugDataItem(std::string_view name, std::string_view sink, std::string_view contents,
                        std::string_view logs) {
    zbi_debugdata_t trailer = {
        .content_size = static_cast<uint32_t>(contents.size()),
        .sink_name_size = static_cast<uint32_t>(sink.size()),
        .vmo_name_size = static_cast<uint32_t>(name.size()),
        .log_size = static_cast<uint32_t>(logs.size()),
    };
    size_t vmo_size =
        sizeof(trailer) + zbitl::AlignedPayloadLength(static_cast<uint32_t>(
                              contents.size() + sink.size() + name.size() + logs.size()));
    std::vector<char> dd_content = {};
    dd_content.resize(vmo_size);
    std::string_view trailer_view{reinterpret_cast<char*>(&trailer), sizeof(trailer)};
    size_t offset = 0;
    for (const auto& content : std::to_array<std::string_view>({contents, sink, name, logs})) {
      content.copy(dd_content.data() + offset, content.size());
      offset += content.size();
    }
    trailer_view.copy(dd_content.data() + dd_content.size() - trailer_view.size(),
                      trailer_view.size());

    zx::vmo debug_data_vmo;
    ASSERT_EQ(zx::vmo::create(vmo_size, 0, &debug_data_vmo), ZX_OK);
    ASSERT_EQ(debug_data_vmo.write(dd_content.data(), 0, dd_content.size()), ZX_OK);
    dd_vmos_.push_back({.vmo = std::move(debug_data_vmo), .len = static_cast<uint32_t>(vmo_size)});
  }

  void Get(fuchsia_boot::wire::ItemsGetRequest* request, GetCompleter::Sync& completer) final {}

  void Get2(fuchsia_boot::wire::ItemsGet2Request* request, Get2Completer::Sync& completer) final {
    if (request->type != ZBI_TYPE_DEBUGDATA) {
      completer.ReplyError(ZX_ERR_NOT_FOUND);
      return;
    }
    std::vector<fuchsia_boot::wire::RetrievedItems> vmos_to_return;
    for (auto& vmo_info : dd_vmos_) {
      zx::vmo dup;
      if (auto res = vmo_info.vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup); res != ZX_OK) {
        completer.ReplyError(res);
        return;
      }
      vmos_to_return.push_back({
          .payload = std::move(dup),
          .length = vmo_info.len,

      });
    }
    completer.ReplySuccess(
        fidl::VectorView<fuchsia_boot::wire::RetrievedItems>::FromExternal(vmos_to_return));
  }

  void GetBootloaderFile(fuchsia_boot::wire::ItemsGetBootloaderFileRequest* request,
                         GetBootloaderFileCompleter::Sync& completer) final {}

  auto take_client_end() { return std::move(svc_channel_); }

 private:
  struct VmoInfo {
    zx::vmo vmo;
    uint32_t len;
  };
  std::vector<VmoInfo> dd_vmos_;
  fidl::ClientEnd<fuchsia_boot::Items> svc_channel_;
};

class ExposeDebugdataZbiItemTest : public testing::Test {
 public:
  void SetUp() final {
    loop_.StartThread("fake-boot-item-thrd");
    server_ = std::make_unique<FakeFuchsiaBootItemServer>(loop_.dispatcher());
  }
  void TearDown() final { loop_.Shutdown(); }
  auto& boot_items() { return *server_; }
  vfs::PseudoDir& logs() { return logs_; }

 private:
  async::Loop loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  std::unique_ptr<FakeFuchsiaBootItemServer> server_;
  vfs::PseudoDir logs_;
};

TEST_F(ExposeDebugdataZbiItemTest, NoDebugDataItems) {
  SinkDirMap sinks;
  auto res = ExposeDebugDataZbiItem(boot_items().take_client_end(), logs(), sinks);
  ASSERT_TRUE(res.is_ok()) << res.status_value();
  EXPECT_EQ(sinks.size(), 0u);
  EXPECT_TRUE(logs().IsEmpty());
}

// Dynamic dir is always empty, look for the vmo_name etc in the
void ValidateExposedBootDebugDataZbiItem(std::string_view vmo_name, std::string_view sink_name,
                                         std::string_view contents, std::string_view logs,
                                         vfs::PseudoDir& log_dir, SinkDirMap& sinks) {
  vfs::Node* sink = nullptr;
  vfs::Node* dyn_sink = nullptr;
  ASSERT_EQ(sinks[std::string(sink_name)]->Lookup(kStaticDir, &sink), ZX_OK);
  ASSERT_EQ(sinks[std::string(sink_name)]->Lookup(kDynamicDir, &dyn_sink), ZX_OK);
  ASSERT_NE(sink, nullptr);
  ASSERT_NE(dyn_sink, nullptr);

  auto* typed_sink = reinterpret_cast<vfs::PseudoDir*>(sink);
  auto* typed_dyn_sink = reinterpret_cast<vfs::PseudoDir*>(dyn_sink);
  EXPECT_TRUE(typed_dyn_sink->IsEmpty());
  vfs::Node* ddfile = nullptr;
  ASSERT_EQ(typed_sink->Lookup(vmo_name, &ddfile), ZX_OK);
  ASSERT_NE(ddfile, nullptr);
  auto* typed_ddfile = reinterpret_cast<vfs::VmoFile*>(ddfile);
  size_t content_size = 0;
  ASSERT_EQ(typed_ddfile->vmo()->get_prop_content_size(&content_size), ZX_OK);
  ASSERT_EQ(content_size, contents.size());

  std::vector<char> contents_buffer;
  contents_buffer.resize(content_size);
  ASSERT_EQ(typed_ddfile->vmo()->read(contents_buffer.data(), 0, content_size), ZX_OK);
  std::string_view content_view(contents_buffer.data(), content_size);
  EXPECT_EQ(content_view, contents);

  std::string log_file_name = std::string(sink_name) + "-" + std::string(vmo_name) + ".log";
  if (logs.empty()) {
    vfs::Node* logfile = nullptr;
    ASSERT_EQ(log_dir.Lookup(log_file_name, &logfile), ZX_ERR_NOT_FOUND);
  } else {
    vfs::Node* logfile = nullptr;
    ASSERT_EQ(log_dir.Lookup(log_file_name, &logfile), ZX_OK);
    ASSERT_NE(logfile, nullptr);

    auto* typed_logfile = reinterpret_cast<vfs::VmoFile*>(logfile);
    size_t log_len = 0;
    ASSERT_EQ(typed_logfile->vmo()->get_prop_content_size(&log_len), ZX_OK);
    EXPECT_EQ(log_len, logs.size());
    std::vector<char> log_content;
    log_content.resize(log_len);
    ASSERT_EQ(typed_logfile->vmo()->read(log_content.data(), 0, log_len), ZX_OK);
    std::string_view log_view(log_content.data(), log_content.size());
    EXPECT_EQ(log_view, logs);
  }
}

TEST_F(ExposeDebugdataZbiItemTest, NoItems) {
  SinkDirMap sinks;
  auto res = ExposeDebugDataZbiItem(boot_items().take_client_end(), logs(), sinks);
  ASSERT_TRUE(res.is_ok()) << res.status_value();
  ASSERT_EQ(sinks.size(), 0u);
  ASSERT_TRUE(logs().IsEmpty());
}

TEST_F(ExposeDebugdataZbiItemTest, SingleItemNoLog) {
  static constexpr std::string_view kVmoName = "foo";
  static constexpr std::string_view kSinkName = "bar";
  static constexpr std::string_view kContents = "foobar";

  SinkDirMap sinks;
  boot_items().AddDebugDataItem(kVmoName, kSinkName, kContents, "");
  auto res = ExposeDebugDataZbiItem(boot_items().take_client_end(), logs(), sinks);
  ASSERT_TRUE(res.is_ok()) << res.status_value();
  ASSERT_EQ(sinks.size(), 1u);
  ValidateExposedBootDebugDataZbiItem(kVmoName, kSinkName, kContents, {}, logs(), sinks);
  ASSERT_TRUE(logs().IsEmpty());
}

TEST_F(ExposeDebugdataZbiItemTest, SingleItemWithLog) {
  static constexpr std::string_view kVmoName = "foo";
  static constexpr std::string_view kSinkName = "bar";
  static constexpr std::string_view kContents = "zoozar";
  static constexpr std::string_view kLogs = "Lorem Ipsum";

  SinkDirMap sinks;
  boot_items().AddDebugDataItem(kVmoName, kSinkName, kContents, kLogs);
  auto res = ExposeDebugDataZbiItem(boot_items().take_client_end(), logs(), sinks);
  ASSERT_TRUE(res.is_ok()) << res.status_value();
  ASSERT_EQ(sinks.size(), 1u);
  ValidateExposedBootDebugDataZbiItem(kVmoName, kSinkName, kContents, kLogs, logs(), sinks);
}

TEST_F(ExposeDebugdataZbiItemTest, MultipleItems) {
  struct DdItem {
    std::string_view vmo;
    std::string_view sink;
    std::string_view content;
    std::string_view logs;
  };

  constexpr auto kItems = std::to_array<DdItem>({
      {
          .vmo = "123",
          .sink = "333",
          .content = "4444",
          .logs = "Foo Bar Ipsum Lorem",
      },
      {
          .vmo = "1234",
          .sink = "222",
          .content = "4444",
          .logs = "Foo Bar Ipsum Lorems",
      },
      {
          .vmo = "1235",
          .sink = "333",
          .content = "4444",
          .logs = {},
      },
      {
          .vmo = "12356",
          .sink = "222",
          .content = "4444",
          .logs = "Foo Bar Ipsum Lorem",
      },
  });

  SinkDirMap sinks;
  for (auto item : kItems) {
    boot_items().AddDebugDataItem(item.vmo, item.sink, item.content, item.logs);
  }
  auto res = ExposeDebugDataZbiItem(boot_items().take_client_end(), logs(), sinks);
  ASSERT_TRUE(res.is_ok()) << res.status_string();
  ASSERT_EQ(sinks.size(), 2u);

  for (auto item : kItems) {
    ValidateExposedBootDebugDataZbiItem(item.vmo, item.sink, item.content, item.logs, logs(),
                                        sinks);
  }
}

}  // namespace
}  // namespace early_boot_instrumentation
