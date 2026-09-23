// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_TESTS_DURATIONS_H_
#define SRC_MEDIA_AUDIO_DRIVERS_TESTS_DURATIONS_H_

#include <lib/zx/time.h>

namespace media::audio::drivers::test {

// Used with WaitForError
inline constexpr zx::duration kWaitForErrorDuration = zx::msec(100);

// Used with CooldownAfterDriverDisconnect
inline constexpr zx::duration kDriverDisconnectCooldownDuration = zx::msec(10);

// Used with CooldownAfterRingBufferDisconnect, which is called after each time that we expect
// our RingBuffer connection to unbind.
inline constexpr zx::duration kRingBufferDisconnectCooldownDuration = zx::msec(100);

// Used with CooldownAfterSignalProcessingDisconnect, which is called after each time that we expect
// our SignalProcessing connection to unbind.
inline constexpr zx::duration kSignalProcessingDisconnectCooldownDuration = zx::msec(25);

// Audio drivers can have multiple StreamConfig channels open, but only one can be 'privileged':
// the one that can in turn create a RingBuffer channel. Each test case starts from scratch,
// opening and closing channels. If we create a StreamConfig channel before the previous one is
// cleared, a new StreamConfig channel will not be privileged, and Admin tests will fail.
//
// When disconnecting a StreamConfig, there's no signal to wait on before proceeding (potentially
// immediately executing other tests); insert a 10-ms wait (needing >3.5ms was never observed).
inline void CooldownAfterDriverDisconnect() {
  zx::nanosleep(zx::deadline_after(kDriverDisconnectCooldownDuration));
}

// When disconnecting a RingBuffer, there's no signal to wait on before proceeding (potentially
// immediately executing other tests); insert a 100-ms wait. This wait is even more important for
// error cases that cause the RingBuffer to disconnect: without it, subsequent test cases that use
// the RingBuffer may receive unexpected errors (e.g. ZX_ERR_PEER_CLOSED or ZX_ERR_INVALID_ARGS).
//
// We need this wait when testing a "real hardware" driver (i.e. on realtime-capable systems). For
// this reason a hardcoded time constant, albeit a test antipattern, is (grudgingly) acceptable.
//
// TODO(https://fxbug.dev/42064975): investigate why we fail without this delay, fix the
// drivers/test as necessary, and eliminate this workaround.
inline void CooldownAfterRingBufferDisconnect() {
  zx::nanosleep(zx::deadline_after(kRingBufferDisconnectCooldownDuration));
}

inline void CooldownAfterSignalProcessingDisconnect() {
  zx::nanosleep(zx::deadline_after(kSignalProcessingDisconnectCooldownDuration));
}

}  // namespace media::audio::drivers::test

#endif  // SRC_MEDIA_AUDIO_DRIVERS_TESTS_DURATIONS_H_
