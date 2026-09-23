// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_USERABI_USERBOOT_TESTS_HELPER_H_
#define ZIRCON_KERNEL_LIB_USERABI_USERBOOT_TESTS_HELPER_H_

#include <lib/zx/channel.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/object.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <cstdint>
#include <string_view>
#include <vector>

struct Message {
  ~Message();

  std::vector<uint8_t> msg;
  std::vector<zx_handle_t> handles;
};

// View over a raw message that returns respective handles.
struct DebugDataMessageView {
  explicit DebugDataMessageView(const Message& msg) : message(&msg) {}

  std::string_view sink() const;
  zx::unowned_vmo vmo() const;
  zx::unowned_eventpair token() const;

  const Message* message = nullptr;
};

struct BootfsFileVmo {
  uint32_t offset;
  zx::vmo contents;
};

// Out parameters are used so that the implementation can use |ASSERT_*| macros and
// verified with |ASSERT_NON_FATAL_FAILURES|.

// Attempts to read a DebugDataMessage from |svc| this include dealing with the pipelined messages.
void GetDebugDataMessage(zx::unowned_channel svc, Message& msg);

// Given a `svc_stash` will attempt to read a stashed svc.
void GetStashedSvc(zx::channel svc_stash, zx::channel& svc_0, zx::channel& svc_1);

inline void GetStashedSvc(zx::channel svc_stash, zx::channel& svc) {
  zx::channel dropped = {};
  GetStashedSvc(std::move(svc_stash), dropped, svc);
}

// Will set `svc_stash` to the equivalent startup SvcStash handle.
// Note: Mutually exclusive with `GetBootfsEnties`.
void GetSvcStash(zx::channel& svc_stash);

//  Will set `entries` to the collection of VMOs posted.
// Note: Mutually exclusive with `GetSvcStash`.
void GetBootfsEntries(std::vector<BootfsFileVmo>& entries);

// `T` is a callable that is given a readable channel and a token to determine whether the loop
// should continue or not. It is important to leave the signature as non returning, such that
// tests may assert in the body.
template <typename T>
void OnEachMessage(zx::unowned_channel svc_stash, T&& callable) {
  zx_signals_t observed = 0;
  while (svc_stash->wait_one(ZX_CHANNEL_READABLE, zx::time::infinite_past(), &observed) !=
         ZX_ERR_TIMED_OUT) {
    if ((observed & ZX_CHANNEL_READABLE) == 0) {
      return;
    }
    bool cont = false;
    if (callable(svc_stash->borrow(), cont); !cont) {
      return;
    }
  }
}

// Returns the kernel object ID of the object and peer object respectively.
zx_koid_t GetKoid(zx_handle_t handle);
zx_koid_t GetPeerKoid(zx_handle_t handle);

#endif  // ZIRCON_KERNEL_LIB_USERABI_USERBOOT_TESTS_HELPER_H_
