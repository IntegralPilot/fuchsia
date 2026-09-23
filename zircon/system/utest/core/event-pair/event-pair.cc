// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/eventpair.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>

#include <zxtest/zxtest.h>

namespace {

zx_signals_t GetPendingSignals(const zx::eventpair& eventpair) {
  zx_signals_t pending = 0;

  EXPECT_STATUS(ZX_ERR_TIMED_OUT, eventpair.wait_one(0, zx::time::infinite_past(), &pending));

  return pending;
}

TEST(EventPairTest, HandlesNotInvalid) {
  zx::eventpair eventpair_0, eventpair_1;

  ASSERT_OK(zx::eventpair::create(0, &eventpair_0, &eventpair_1));

  EXPECT_NE(eventpair_0.get(), ZX_HANDLE_INVALID);
  EXPECT_NE(eventpair_1.get(), ZX_HANDLE_INVALID);
}

TEST(EventPairTest, HandleRightsAreCorrect) {
  zx::eventpair eventpair_0, eventpair_1;

  ASSERT_OK(zx::eventpair::create(0, &eventpair_0, &eventpair_1));

  zx_info_handle_basic_t info = {};
  ASSERT_OK(eventpair_0.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.rights, ZX_RIGHTS_BASIC | ZX_RIGHT_SIGNAL | ZX_RIGHT_SIGNAL_PEER);
  EXPECT_EQ(info.type, static_cast<uint32_t>(ZX_OBJ_TYPE_EVENTPAIR));

  info = {};
  ASSERT_OK(eventpair_1.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.rights, ZX_RIGHTS_BASIC | ZX_RIGHT_SIGNAL | ZX_RIGHT_SIGNAL_PEER);
  EXPECT_EQ(info.type, static_cast<uint32_t>(ZX_OBJ_TYPE_EVENTPAIR));
}

TEST(EventPairTest, KoidsAreCorrect) {
  zx::eventpair eventpair_0, eventpair_1;

  ASSERT_OK(zx::eventpair::create(0, &eventpair_0, &eventpair_1));

  zx_info_handle_basic_t info_0 = {};
  zx_info_handle_basic_t info_1 = {};
  ASSERT_OK(eventpair_0.get_info(ZX_INFO_HANDLE_BASIC, &info_0, sizeof(info_0), nullptr, nullptr));
  ASSERT_OK(eventpair_1.get_info(ZX_INFO_HANDLE_BASIC, &info_1, sizeof(info_1), nullptr, nullptr));

  // Check that koids line up.
  EXPECT_NE(info_0.koid, 0);
  EXPECT_NE(info_0.related_koid, 0);
  EXPECT_NE(info_1.koid, 0);
  EXPECT_NE(info_1.related_koid, 0);
  EXPECT_EQ(info_0.koid, info_1.related_koid);
  EXPECT_EQ(info_1.koid, info_0.related_koid);
}

// Currently no flags are supported.
TEST(EventPairTest, CheckNoFlagsSupported) {
  zx::eventpair eventpair_0, eventpair_1;

  ASSERT_STATUS(ZX_ERR_NOT_SUPPORTED, zx::eventpair::create(1, &eventpair_0, &eventpair_1));

  EXPECT_EQ(eventpair_0.get(), ZX_HANDLE_INVALID);
  EXPECT_EQ(eventpair_1.get(), ZX_HANDLE_INVALID);
}

TEST(EventPairTest, SignalEventPairAndClearVerifySignals) {
  zx::eventpair eventpair_0, eventpair_1;

  ASSERT_OK(zx::eventpair::create(0, &eventpair_0, &eventpair_1));

  EXPECT_EQ(GetPendingSignals(eventpair_0), 0);
  EXPECT_EQ(GetPendingSignals(eventpair_1), 0);

  ASSERT_OK(eventpair_0.signal(0, ZX_USER_SIGNAL_0));
  EXPECT_EQ(GetPendingSignals(eventpair_0), ZX_USER_SIGNAL_0);
  EXPECT_EQ(GetPendingSignals(eventpair_1), 0);

  ASSERT_OK(eventpair_0.signal(ZX_USER_SIGNAL_0, 0));
  EXPECT_EQ(GetPendingSignals(eventpair_1), 0);
  EXPECT_EQ(GetPendingSignals(eventpair_0), 0);
}

TEST(EventPairTest, SignalPeerAndVerifyRecived) {
  zx::eventpair eventpair_0, eventpair_1;

  ASSERT_OK(zx::eventpair::create(0, &eventpair_0, &eventpair_1));

  ASSERT_OK(eventpair_0.signal_peer(0, ZX_USER_SIGNAL_0));
  EXPECT_EQ(GetPendingSignals(eventpair_0), 0);
  EXPECT_EQ(GetPendingSignals(eventpair_1), ZX_USER_SIGNAL_0);

  ASSERT_OK(eventpair_1.signal_peer(0, ZX_USER_SIGNAL_1 | ZX_USER_SIGNAL_2));
  EXPECT_EQ(GetPendingSignals(eventpair_0), ZX_USER_SIGNAL_1 | ZX_USER_SIGNAL_2);
  EXPECT_EQ(GetPendingSignals(eventpair_1), ZX_USER_SIGNAL_0);

  ASSERT_OK(eventpair_0.signal_peer(ZX_USER_SIGNAL_0, ZX_USER_SIGNAL_3 | ZX_USER_SIGNAL_4));
  EXPECT_EQ(GetPendingSignals(eventpair_0), ZX_USER_SIGNAL_1 | ZX_USER_SIGNAL_2);
  EXPECT_EQ(GetPendingSignals(eventpair_1), ZX_USER_SIGNAL_3 | ZX_USER_SIGNAL_4);
}

TEST(EventPairTest, SignalPeerThenCloseAndVerifySignalReceived) {
  zx::eventpair eventpair_0, eventpair_1;

  ASSERT_OK(zx::eventpair::create(0, &eventpair_0, &eventpair_1));

  ASSERT_OK(eventpair_0.signal_peer(0, ZX_USER_SIGNAL_3 | ZX_USER_SIGNAL_4));

  eventpair_0.reset();

  // Signaled flags should remain satisfied but now should now also get peer closed (and
  // unsignaled flags should be unsatisfiable).
  EXPECT_EQ(GetPendingSignals(eventpair_1),
            ZX_EVENTPAIR_PEER_CLOSED | ZX_USER_SIGNAL_3 | ZX_USER_SIGNAL_4);
}

TEST(EventPairTest, SignalingClosedPeerReturnsPeerClosed) {
  zx::eventpair eventpair_0, eventpair_1;

  ASSERT_OK(zx::eventpair::create(0, &eventpair_0, &eventpair_1));

  eventpair_1.reset();
  EXPECT_STATUS(ZX_ERR_PEER_CLOSED, eventpair_0.signal_peer(0, ZX_USER_SIGNAL_0));
}

TEST(EventPairTest, SignalAccessDeniedMissingSignalRight) {
  zx::eventpair eventpair_0, eventpair_1;

  ASSERT_OK(zx::eventpair::create(0, &eventpair_0, &eventpair_1));

  zx_info_handle_basic_t info = {};
  ASSERT_OK(eventpair_0.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));

  zx::eventpair reduced_eventpair;
  ASSERT_OK(eventpair_0.duplicate(info.rights & ~ZX_RIGHT_SIGNAL, &reduced_eventpair));

  EXPECT_STATUS(ZX_ERR_ACCESS_DENIED, reduced_eventpair.signal(0, ZX_USER_SIGNAL_0));
}

TEST(EventPairTest, SignalPeerAccessDeniedMissingSignalPeerRight) {
  zx::eventpair eventpair_0, eventpair_1;

  ASSERT_OK(zx::eventpair::create(0, &eventpair_0, &eventpair_1));

  zx_info_handle_basic_t info = {};
  ASSERT_OK(eventpair_0.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));

  zx::eventpair reduced_eventpair;
  ASSERT_OK(eventpair_0.duplicate(info.rights & ~ZX_RIGHT_SIGNAL_PEER, &reduced_eventpair));

  EXPECT_STATUS(ZX_ERR_ACCESS_DENIED, reduced_eventpair.signal_peer(0, ZX_USER_SIGNAL_0));
}

TEST(EventPairTest, WaitAccessDeniedMissingWaitRight) {
  zx::eventpair eventpair_0, eventpair_1;

  ASSERT_OK(zx::eventpair::create(0, &eventpair_0, &eventpair_1));

  zx_info_handle_basic_t info = {};
  ASSERT_OK(eventpair_0.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));

  zx::eventpair reduced_eventpair;
  ASSERT_OK(eventpair_0.duplicate(info.rights & ~ZX_RIGHT_WAIT, &reduced_eventpair));

  zx_signals_t pending;
  EXPECT_STATUS(ZX_ERR_ACCESS_DENIED,
                reduced_eventpair.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite_past(), &pending));
}

TEST(EventPairTest, DuplicateHandlesAndPeerClosed) {
  zx::eventpair eventpair_0, eventpair_1;
  ASSERT_OK(zx::eventpair::create(0, &eventpair_0, &eventpair_1));

  zx::eventpair dup_0a, dup_0b;
  ASSERT_OK(eventpair_0.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup_0a));
  ASSERT_OK(eventpair_0.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup_0b));

  // Resetting original handle should not trigger peer closed while duplicates exist.
  eventpair_0.reset();
  EXPECT_EQ(GetPendingSignals(eventpair_1), 0);

  // Resetting first duplicate still leaves second duplicate alive.
  dup_0a.reset();
  EXPECT_EQ(GetPendingSignals(eventpair_1), 0);

  // Surviving duplicate can still signal peer.
  ASSERT_OK(dup_0b.signal_peer(0, ZX_USER_SIGNAL_0));
  EXPECT_EQ(GetPendingSignals(eventpair_1), ZX_USER_SIGNAL_0);

  // Resetting last handle triggers peer closed on peer endpoint.
  dup_0b.reset();
  EXPECT_EQ(GetPendingSignals(eventpair_1), ZX_EVENTPAIR_PEER_CLOSED | ZX_USER_SIGNAL_0);
}

TEST(EventPairTest, SignalSelfAfterPeerClosed) {
  zx::eventpair eventpair_0, eventpair_1;
  ASSERT_OK(zx::eventpair::create(0, &eventpair_0, &eventpair_1));

  eventpair_1.reset();
  EXPECT_EQ(GetPendingSignals(eventpair_0), ZX_EVENTPAIR_PEER_CLOSED);

  // Surviving endpoint can still modify its own user signals.
  ASSERT_OK(eventpair_0.signal(0, ZX_USER_SIGNAL_3));
  EXPECT_EQ(GetPendingSignals(eventpair_0), ZX_EVENTPAIR_PEER_CLOSED | ZX_USER_SIGNAL_3);

  ASSERT_OK(eventpair_0.signal(ZX_USER_SIGNAL_3, 0));
  EXPECT_EQ(GetPendingSignals(eventpair_0), ZX_EVENTPAIR_PEER_CLOSED);
}

TEST(EventPairTest, InvalidSignalBits) {
  zx::eventpair eventpair_0, eventpair_1;
  ASSERT_OK(zx::eventpair::create(0, &eventpair_0, &eventpair_1));

  // ZX_EVENTPAIR_PEER_CLOSED cannot be asserted or cleared by userspace.
  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, eventpair_0.signal(0, ZX_EVENTPAIR_PEER_CLOSED));
  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, eventpair_0.signal(ZX_EVENTPAIR_PEER_CLOSED, 0));
  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, eventpair_0.signal_peer(0, ZX_EVENTPAIR_PEER_CLOSED));
  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, eventpair_0.signal_peer(ZX_EVENTPAIR_PEER_CLOSED, 0));

  // Non-allowed signal bits (outside ZX_USER_SIGNAL_ALL | ZX_EVENT_SIGNALED) return
  // ZX_ERR_INVALID_ARGS.
  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, eventpair_0.signal(0, 1u << 4));
  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, eventpair_0.signal_peer(0, 1u << 4));
  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, eventpair_0.signal(0, 1u << 23));
  EXPECT_STATUS(ZX_ERR_INVALID_ARGS, eventpair_0.signal_peer(0, 1u << 23));

  // ZX_EVENT_SIGNALED is allowed.
  ASSERT_OK(eventpair_0.signal(0, ZX_EVENT_SIGNALED));
  EXPECT_EQ(GetPendingSignals(eventpair_0), ZX_EVENT_SIGNALED);
  ASSERT_OK(eventpair_0.signal(ZX_EVENT_SIGNALED, 0));
  EXPECT_EQ(GetPendingSignals(eventpair_0), 0);

  ASSERT_OK(eventpair_0.signal_peer(0, ZX_EVENT_SIGNALED));
  EXPECT_EQ(GetPendingSignals(eventpair_1), ZX_EVENT_SIGNALED);
  ASSERT_OK(eventpair_0.signal_peer(ZX_EVENT_SIGNALED, 0));
  EXPECT_EQ(GetPendingSignals(eventpair_1), 0);
}

TEST(EventPairTest, InvalidHandleAndWrongType) {
  // Invalid handle
  EXPECT_STATUS(ZX_ERR_BAD_HANDLE, zx_object_signal_peer(ZX_HANDLE_INVALID, 0, ZX_USER_SIGNAL_0));

  // Non-peered object type without ZX_RIGHT_SIGNAL_PEER (event) returns ZX_ERR_ACCESS_DENIED
  zx_handle_t event = ZX_HANDLE_INVALID;
  ASSERT_OK(zx_event_create(0, &event));
  EXPECT_STATUS(ZX_ERR_ACCESS_DENIED, zx_object_signal_peer(event, 0, ZX_USER_SIGNAL_0));
  zx_handle_close(event);
}

}  // namespace
