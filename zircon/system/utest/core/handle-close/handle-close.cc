// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/eventpair.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/object.h>

#include <fbl/vector.h>
#include <zxtest/zxtest.h>

namespace {

constexpr uint32_t kNumEventpairCombos = 4u;
constexpr uint32_t kNumEventpairsInvalid = 2u;
constexpr uint32_t kOptions = 0u;

void PeerWasClosed(const zx::eventpair& eventpair) {
  zx_signals_t signals;
  ASSERT_OK(eventpair.wait_one(ZX_EVENTPAIR_PEER_CLOSED, zx::time(), &signals));
  ASSERT_EQ(signals & ZX_EVENTPAIR_PEER_CLOSED, ZX_EVENTPAIR_PEER_CLOSED);
}

TEST(HandleCloseTest, Many) {
  zx::eventpair eventpair_0[kNumEventpairCombos];
  zx::eventpair eventpair_1[kNumEventpairCombos];
  zx_handle_t handles[kNumEventpairCombos] = {};

  for (size_t idx = 0u; idx < kNumEventpairCombos; ++idx) {
    ASSERT_OK(zx::eventpair::create(kOptions, &eventpair_0[idx], &eventpair_1[idx]));
    // We don't transfer ownership, just in case close many fails, and we can try
    // closing each handle individually when the test scope exits.
    handles[idx] = eventpair_0[idx].get();
  }
  // Close all of the handles from eventpair_0.
  ASSERT_OK(zx_handle_close_many(handles, kNumEventpairCombos));

  // Verify all the peers of the eventpair were indeed closed.
  for (const auto& eventpair : eventpair_1) {
    ASSERT_NO_FATAL_FAILURE(PeerWasClosed(eventpair));
  }
}

TEST(HandleCloseTest, ManyInvalidHandlesShouldNotFail) {
  // The handles layout: 0 1 2 3 : invalid invalid : 0 1 2 3
  zx::eventpair eventpair_0[kNumEventpairCombos];
  zx::eventpair eventpair_1[kNumEventpairCombos];
  zx_handle_t handles[kNumEventpairCombos + kNumEventpairsInvalid] = {ZX_HANDLE_INVALID};

  for (size_t idx = 0u; idx < kNumEventpairCombos; ++idx) {
    ASSERT_OK(zx::eventpair::create(kOptions, &eventpair_0[idx], &eventpair_1[idx]));
    // We don't transfer ownership, just in case close many fails, and we can try
    // closing each handle individually when the test scope exits.
    handles[idx] = eventpair_0[idx].get();
  }

  // This invokes close_many with the first 4 valid handles, plus the
  // next two invalid handles, and should close all without failure.
  ASSERT_OK(zx_handle_close_many(handles, kNumEventpairCombos + kNumEventpairsInvalid));

  // Verify all the peers of the eventpair were indeed closed.
  for (const auto& eventpair : eventpair_1) {
    ASSERT_NO_FATAL_FAILURE(PeerWasClosed(eventpair));
  }
}

TEST(HandleCloseTest, ManyDuplicateTest) {
  // The handles layout: 0 1 0 1 2 3 : 0 1 2 3
  zx::eventpair eventpair_0[kNumEventpairCombos];
  zx::eventpair eventpair_1[kNumEventpairCombos];
  zx_handle_t handles[kNumEventpairCombos + kNumEventpairsInvalid] = {};

  for (size_t idx = 0u; idx < kNumEventpairCombos; ++idx) {
    ASSERT_OK(zx::eventpair::create(kOptions, &eventpair_0[idx], &eventpair_1[idx]));
    // We don't transfer ownership, just in case close many fails, and we can try
    // closing each handle individually when the test scope exits.
    handles[idx + kNumEventpairsInvalid] = eventpair_0[idx].get();
  }

  // Duplicate the values at the start.
  handles[0u] = handles[kNumEventpairsInvalid];
  handles[1u] = handles[kNumEventpairsInvalid + 1u];

  // This returns an error value: the duplicated handles
  // can't be closed twice. Despite this, all handles were closed.
  ASSERT_EQ(zx_handle_close_many(handles, kNumEventpairCombos + kNumEventpairsInvalid),
            ZX_ERR_BAD_HANDLE);

  // Assert that every handle in the preceding close call was in
  // fact closed, by waiting on the PEER_CLOSED signal.
  for (const auto& eventpair : eventpair_1) {
    ASSERT_NO_FATAL_FAILURE(PeerWasClosed(eventpair));
  }
}

TEST(HandleCloseTest, RegressionTest_479281267) {
  // This is a regression test for https://issuetracker.google.com/479281267
  //
  // There is no upper limit on the number of handles which can be closed using
  // a call to `zx_handle_close_many`.  But, because of implementation details
  // involving the limited size of kernel stacks, handle closures are processed
  // in chunks of (at most) ZX_CHANNEL_MAX_MSG_HANDLES.
  //
  // Prior to the fix, if the first batch of handles had a bad handle in it (not
  // ZX_HANDLE_INVALID, but a handle value which was already closed or simply
  // corrupted), but the second batch was all OK, an incorrect status of ZX_OK
  // would be returned instead of ZX_ERR_BAD_HANDLE as it should be.
  //
  // Explicitly check this.  Send a close operation with enough handles to
  // exceed the batching limit, and make sure that even if the first batch has a
  // bad handle while the second does not, that:
  //
  // 1) ZX_ERR_BAD_HANDLE is returned as it should be.
  // 2) All of the "good" handles are proper closed.
  //
  // Reserve an extra handle in our `to_close` handle array, and populate it with a
  // bad handle.  This will force the kernel to process the operation in two
  // batches, where the first batch has a bad handle but the second does not.
  //
  std::array<zx_handle_t, ZX_CHANNEL_MAX_MSG_HANDLES + 1> to_close;
  std::array<zx::eventpair, ZX_CHANNEL_MAX_MSG_HANDLES> eventpair_0;
  std::array<zx::eventpair, ZX_CHANNEL_MAX_MSG_HANDLES> eventpair_1;

  for (size_t idx = 0u; idx < eventpair_0.size(); ++idx) {
    ASSERT_OK(zx::eventpair::create(kOptions, &eventpair_0[idx], &eventpair_1[idx]));

    // Go ahead and release ownership of one half of the eventpair into
    // to_close.  Our operation is _supposed_ to close the handle for us, and it
    // seems inappropriate to double close the handle in a successful test case.
    //
    // This would be, strictly speaking, a violation of bad-handle policy (even
    // if core tests do not run with bad-handle policy turned on).  If the
    // operation fails to do its job (and does not close all of the handles) so
    // be it.  The test will fail when we check to be sure that all of our
    // event-pairs have been properly closed, so we won't miss this error, and
    // the handles will be cleaned up when the test process is closed.
    to_close[idx + 1] = eventpair_0[idx].release();
  }

  // 0xbadc0ffe is always a bad handle value since bit 0 of a valid handle value
  // must always be 1.
  to_close[0] = 0xbadc0ffe;

  // Our first batch has a bad handle which should return an error.
  // Despite this, all handles good handles were closed.
  ASSERT_EQ(zx_handle_close_many(to_close.data(), to_close.size()), ZX_ERR_BAD_HANDLE);
  for (const auto& eventpair : eventpair_1) {
    ASSERT_NO_FATAL_FAILURE(PeerWasClosed(eventpair));
  }
}

}  // namespace
