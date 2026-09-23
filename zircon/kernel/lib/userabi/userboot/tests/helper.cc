// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "helper.h"

#include <fidl/fuchsia.boot/cpp/markers.h>
#include <fidl/fuchsia.boot/cpp/wire.h>
#include <fidl/fuchsia.boot/cpp/wire_types.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/fidl/cpp/wire/wire_messaging.h>
#include <lib/standalone-test/standalone.h>
#include <lib/zx/channel.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/dlfcn.h>
#include <zircon/fidl.h>
#include <zircon/process.h>
#include <zircon/processargs.h>
#include <zircon/types.h>

#include <algorithm>
#include <vector>

#include <zxtest/zxtest.h>

namespace {

class UserbootServer final : public fidl::WireServer<fuchsia_boot::Userboot> {
 public:
  void PostStashSvc(PostStashSvcRequestView request, PostStashSvcCompleter::Sync& completer) final {
    stash_svc_ = request->stash_svc_endpoint.TakeChannel();
  }

  void PostBootfsFiles(PostBootfsFilesRequestView request,
                       PostBootfsFilesCompleter::Sync& completer) final {
    for (auto& entry : request->files) {
      entries_.emplace_back(BootfsFileVmo{
          .offset = entry.offset,
          .contents = std::move(entry.contents),
      });
    }
  }

  zx::channel take_stash_svc() { return std::move(stash_svc_); }
  std::vector<BootfsFileVmo> take_bootfs_entries() { return std::move(entries_); }

 private:
  zx::channel stash_svc_;
  std::vector<BootfsFileVmo> entries_;
};

class StashSvcServer final : public fidl::WireServer<fuchsia_boot::SvcStash> {
 public:
  void Store(StoreRequestView request, StoreCompleter::Sync& completer) final {
    stashed_svc_.push_back(request->svc_endpoint.TakeChannel());
  }

  auto& stashed_svcs() { return stashed_svc_; }

 private:
  std::vector<zx::channel> stashed_svc_;
};

// TODO(https://fxbug.dev/42072759): Replace copy & pasted FIDL C bindings with new C++ bindings
// when that's allowed.
struct fuchsia_debugdata_PublisherPublishRequestMessage {
  FIDL_ALIGNDECL
  fidl_message_header_t hdr;
  fidl_string_t data_sink;
  zx_handle_t data;
  zx_handle_t vmo_token;
};

}  // namespace

zx_koid_t GetKoid(zx_handle_t handle) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  EXPECT_OK(status);
  return status == ZX_OK ? info.koid : ZX_KOID_INVALID;
}

zx_koid_t GetPeerKoid(zx_handle_t handle) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  EXPECT_OK(status);
  return status == ZX_OK ? info.related_koid : ZX_KOID_INVALID;
}

Message::~Message() { zx_handle_close_many(handles.data(), handles.size()); }

std::string_view DebugDataMessageView::sink() const {
  const auto* publish_rq =
      reinterpret_cast<const fuchsia_debugdata_PublisherPublishRequestMessage*>(
          message->msg.data());
  ZX_ASSERT(message->msg.size() >= publish_rq->data_sink.size + sizeof(*publish_rq));

  return {reinterpret_cast<const char*>(message->msg.data() + sizeof(*publish_rq)),
          publish_rq->data_sink.size};
}

zx::unowned_vmo DebugDataMessageView::vmo() const {
  ZX_ASSERT(message->handles.size() >= 1);
  return zx::unowned_vmo(message->handles[0]);
}

zx::unowned_eventpair DebugDataMessageView::token() const {
  ZX_ASSERT(message->handles.size() >= 2);
  return zx::unowned_eventpair(message->handles[1]);
}

void GetSvcStash(zx::channel& svc_stash) {
  auto userboot = zx::channel(zx_take_startup_handle(PA_HND(PA_USER0, 0)));
  // Drain messages, until you find the Post message for SvcStash.
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  UserbootServer server;
  fidl::ServerEnd<fuchsia_boot::Userboot> userboot_endpoint(std::move(userboot));
  fidl::BindServer(loop.dispatcher(), std::move(userboot_endpoint), &server);
  // Drain all messages.
  loop.RunUntilIdle();

  svc_stash = server.take_stash_svc();
  ASSERT_TRUE(svc_stash.is_valid());
}

void GetStashedSvc(zx::channel svc_stash, zx::channel& svc_0, zx::channel& svc_1) {
  ASSERT_TRUE(svc_stash.is_valid());
  // Drain messages, until you find the Post message for SvcStash.
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  StashSvcServer server;
  fidl::ServerEnd<fuchsia_boot::SvcStash> svc_stash_endpoint(std::move(svc_stash));
  fidl::BindServer(loop.dispatcher(), std::move(svc_stash_endpoint), &server);
  // Drain all messages.
  loop.RunUntilIdle();

  auto& svcs = server.stashed_svcs();
  ASSERT_GT(svcs.size(), 0);
  svc_1 = std::move(svcs.back());
  ASSERT_TRUE(svc_1.is_valid());
  svcs.pop_back();
  if (!svcs.empty()) {
    svc_0 = std::move(svcs.back());
    ASSERT_TRUE(svc_0.is_valid());
    svcs.pop_back();
  }
  ASSERT_EQ(svcs.size(), 0u);
}

void GetDebugDataMessage(zx::unowned_channel svc, Message& msg) {
  zx_signals_t observed = 0;
  // The channel must have contents or we will block for ever.
  ASSERT_OK(svc->wait_one(ZX_CHANNEL_READABLE, zx::time::infinite_past(), &observed));
  ASSERT_TRUE((observed & ZX_CHANNEL_READABLE) != 0);

  // There should be an open request with the server side of the Publisher protocol.
  uint32_t actual_bytes, actual_handles;
  ASSERT_NOT_OK(svc->read(0, nullptr, nullptr, 0, 0, &actual_bytes, &actual_handles));

  ASSERT_GT(actual_bytes, 0);
  ASSERT_GT(actual_handles, 0);

  msg.msg.resize(actual_bytes);
  msg.handles.resize(actual_handles);

  std::fill(msg.msg.begin(), msg.msg.end(), 0);
  std::fill(msg.handles.begin(), msg.handles.end(), ZX_HANDLE_INVALID);

  ASSERT_OK(svc->read(0, msg.msg.data(), msg.handles.data(), static_cast<uint32_t>(msg.msg.size()),
                      static_cast<uint32_t>(msg.handles.size()), &actual_bytes, &actual_handles));

  ASSERT_EQ(actual_bytes, msg.msg.size());
  ASSERT_EQ(actual_handles, msg.handles.size());
  ASSERT_EQ(actual_handles, 1);

  // There should be an attached channel.
  zx_info_handle_basic_t debug_data_channel_info = {};
  zx::handle debug_data_channel(msg.handles[0]);
  ASSERT_OK(debug_data_channel.get_info(ZX_INFO_HANDLE_BASIC, &debug_data_channel_info,
                                        sizeof(debug_data_channel_info), nullptr, nullptr));
  ASSERT_EQ(debug_data_channel_info.type, ZX_OBJ_TYPE_CHANNEL);

  // Now check the contents of the message itself, such that the vmo and name match.
  msg.msg.clear();
  msg.handles.clear();

  // Check that there are messages, and read them.
  zx::channel debug_data(debug_data_channel.release());
  // The channel must have contents or we will block for ever.
  ASSERT_OK(debug_data.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite_past(), &observed));
  ASSERT_TRUE((observed & ZX_CHANNEL_READABLE) != 0);

  ASSERT_NOT_OK(debug_data.read(0, nullptr, nullptr, 0, 0, &actual_bytes, &actual_handles));

  msg.msg.resize(actual_bytes);
  msg.handles.resize(actual_handles);
  std::fill(msg.msg.begin(), msg.msg.end(), 0);
  std::fill(msg.handles.begin(), msg.handles.end(), ZX_HANDLE_INVALID);

  ASSERT_OK(
      debug_data.read(0, msg.msg.data(), msg.handles.data(), static_cast<uint32_t>(msg.msg.size()),
                      static_cast<uint32_t>(msg.handles.size()), &actual_bytes, &actual_handles));
  auto* publish_rq =
      reinterpret_cast<fuchsia_debugdata_PublisherPublishRequestMessage*>(msg.msg.data());
  ASSERT_GE(actual_bytes, sizeof(*publish_rq));
  // 0 -> data, 1 -> token(event pair from the one returned in llvm publish data)
  ASSERT_EQ(actual_handles, 2);
  ASSERT_GE(actual_bytes, publish_rq->data_sink.size + sizeof(*publish_rq));
  ASSERT_NE(msg.handles[0], ZX_HANDLE_INVALID);
  ASSERT_NE(msg.handles[1], ZX_HANDLE_INVALID);
}

void GetBootfsEntries(std::vector<BootfsFileVmo>& entries) {
  auto userboot = zx::channel(zx_take_startup_handle(PA_HND(PA_USER0, 0)));
  ASSERT_TRUE(userboot);

  // Wait until userboot closes their channel endpoint, signalling that messages have been written.
  zx_signals_t signals;
  // Close the loader service channel, such that userboot publishes any remaining message.
  zx_handle_close(dl_set_loader_service(ZX_HANDLE_INVALID));
  // Wait until userboot closes the userboot channel end.
  ASSERT_OK(userboot.wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), &signals));
  ASSERT_TRUE((signals & ZX_CHANNEL_PEER_CLOSED) != 0);

  // Drain messages, until you find the Post message for SvcStash.
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  UserbootServer server;

  fidl::ServerEnd<fuchsia_boot::Userboot> userboot_endpoint(std::move(userboot));
  fidl::BindServer(loop.dispatcher(), std::move(userboot_endpoint), &server);
  // Drain all messages.
  loop.RunUntilIdle();
  entries = server.take_bootfs_entries();
  ASSERT_GT(entries.size(), 0);
}
