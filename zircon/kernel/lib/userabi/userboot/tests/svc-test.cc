// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/standalone-test/standalone.h>
#include <lib/zx/channel.h>
#include <lib/zx/eventpair.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/sanitizer.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <array>
#include <cstdint>
#include <string_view>

#include <zxtest/zxtest.h>

#include "helper.h"

namespace {

class SvcSingleProcessTest : public zxtest::Test {
 public:
  void SetUp() final {
    static zx::channel svc_stash;
    static zx::channel svc_server;

    // Prevents multiple iterations from discarding this.
    if (!svc_server) {
      GetSvcStash(svc_stash);
      ASSERT_TRUE(svc_stash);
    }

    // Order matters, provider is stored first.
    if (svc_stash.is_valid()) {
      ASSERT_NO_FATAL_FAILURE(GetStashedSvc(std::move(svc_stash), svc_server));
    }

    svc_ = standalone::GetNsDir("/svc");
    stashed_svc_ = svc_server.borrow();

    // Drain any preexisting messages from stashed svc, such that we can properly watch the state
    // transition. In some instrumented builds, we may have provided actual debug data.
    // This test expect that there is 'nothing' in the stash, so lets pretend that.
    uint32_t actual_bytes = 0;
    uint32_t actual_handles = 0;

    zx_signals_t observed = 0;
    while (stashed_svc()->wait_one(ZX_CHANNEL_READABLE, zx::time::infinite_past(), &observed) !=
           ZX_ERR_TIMED_OUT) {
      ASSERT_TRUE((observed & ZX_CHANNEL_READABLE) != 0);
      // We are peeking into the channel.
      ASSERT_NOT_OK(stashed_svc()->read(0, nullptr, nullptr, 0, 0, &actual_bytes, &actual_handles));
      std::vector<uint8_t> bytes(actual_bytes, 0);
      std::vector<zx_handle_t> handles(actual_handles, ZX_HANDLE_INVALID);
      ASSERT_OK(stashed_svc()->read(
          0, bytes.data(), handles.data(), static_cast<uint32_t>(bytes.size()),
          static_cast<uint32_t>(handles.size()), &actual_bytes, &actual_handles));
      observed = 0;
    }
  }

  zx::unowned_channel local_svc() { return svc_->borrow(); }
  zx::unowned_channel stashed_svc() { return stashed_svc_->borrow(); }

 private:
  zx::unowned_channel svc_;
  zx::unowned_channel stashed_svc_;
};

TEST_F(SvcSingleProcessTest, SvcStubIsValidHandle) { ASSERT_TRUE(local_svc()->is_valid()); }

TEST_F(SvcSingleProcessTest, WritingIntoSvcShowsUpInStashHandle) {
  ASSERT_TRUE(local_svc()->is_valid());
  ASSERT_TRUE(stashed_svc()->is_valid());

  auto written_message = std::to_array("Hello World!");
  ASSERT_OK(local_svc()->write(0, written_message.data(), written_message.size(), nullptr, 0));
  decltype(written_message) read_message = {};
  uint32_t actual_bytes, actual_handles;
  ASSERT_OK(stashed_svc()->read(0, read_message.data(), nullptr, read_message.size(), 0,
                                &actual_bytes, &actual_handles));

  EXPECT_EQ(read_message, written_message);
}

TEST_F(SvcSingleProcessTest, SanitizerPublishDataShowsUpInStashedHandle) {
  ASSERT_TRUE(local_svc()->is_valid());
  ASSERT_TRUE(stashed_svc()->is_valid());

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(124, 0, &vmo));

  // Channel transitions from not readable to readable.
  zx_signals_t observed = 0;
  ASSERT_STATUS(stashed_svc()->wait_one(ZX_CHANNEL_READABLE, zx::time::infinite_past(), &observed),
                ZX_ERR_TIMED_OUT);

  constexpr const char* kDataSink = "some_sink_name";
  zx_koid_t vmo_koid = GetKoid(vmo.get());
  zx::eventpair token_1(__sanitizer_publish_data(kDataSink, vmo.release()));
  ASSERT_TRUE(token_1);

  zx_koid_t token_koid = GetPeerKoid(token_1.get());
  ASSERT_NE(token_koid, ZX_KOID_INVALID);
  ASSERT_NE(vmo_koid, ZX_KOID_INVALID);

  bool found = false;
  OnEachMessage(stashed_svc(), [&](zx::unowned_channel stashed_svc, bool& keep_going) {
    keep_going = false;
    Message debug_msg;
    ASSERT_NO_FAILURES(GetDebugDataMessage(std::move(stashed_svc), debug_msg));
    DebugDataMessageView view(debug_msg);
    if (view.sink() != kDataSink) {
      keep_going = true;
      return;
    }
    ASSERT_EQ(GetKoid(view.vmo()->get()), vmo_koid);
    ASSERT_EQ(GetKoid(view.token()->get()), token_koid);
    found = true;
  });
  ASSERT_TRUE(found);
}

}  // namespace
