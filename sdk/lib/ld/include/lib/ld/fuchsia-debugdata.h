// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_FUCHSIA_DEBUGDATA_H_
#define LIB_LD_FUCHSIA_DEBUGDATA_H_

#include <lib/zx/channel.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <string_view>

namespace ld {

// This encapsulates what are essentially lightweight FIDL bindings for
// fuchsia.debugdata.Publisher/Publish and a couple of FIDL messages entailed
// in using it.

class Debugdata {
 public:
  struct Deferred {
    zx::eventpair vmo_token;
    zx::channel svc_server_end;
  };

  Debugdata() = default;
  Debugdata(Debugdata&&) = default;

  // The sink name and VMO are as described in the FIDL protocol's comments:
  // https://fuchsia.dev/reference/fidl/fuchsia.debugdata?hl=en#publisher
  Debugdata(std::string_view data_sink, zx::vmo data)
      : data_sink_{data_sink}, data_{std::move(data)} {}

  Debugdata& operator=(Debugdata&&) = default;

  // Send the fuchsia.debugdata.Publisher/Publish message in a pipelined open
  // on the "/svc" name table entry's client side, consuming this object.  The
  // returned eventpair acts as a token; its lifetime represents the continued
  // mutability of the data in the VMO.  If the VMO is mapped in and will be
  // touched until this process exits, then the handle can just be leaked with
  // a release() call.  Otherwise it should be closed as soon as the VMO is no
  // longer being modified and not before (more or less), enabling the receiver
  // of the data to begin consuming it as soon as they are ready.
  zx::result<zx::eventpair> Publish(zx::unowned_channel svc_client_end) &&;

  // Consume the object like Publish but stuff the pipelined open message meant
  // for the "/svc" name table entry into the client end of a new channel, and
  // return its server end and token.  This defers the actual transfer until
  // later, when the name table is available.  At that time, the server end can
  // be passed to Forward.
  zx::result<Deferred> DeferredPublish() &&;

  // When the Deferred::svc_server_end from a DeferredPublish call is plumbed
  // to where the svc_client_end is available, this forwards it roughly as if
  // Publish had been called on the original object instead of DeferredPublish.
  // (The Deferred::vmo_token from the earlier call must be plumbed separately
  // or has already been intentionally leaked or closed.)
  static zx::result<> Forward(zx::unowned_channel svc_client_end, zx::channel svc_server_end);

 private:
  std::string_view data_sink_;
  zx::vmo data_;
};

}  // namespace ld

#endif  // LIB_LD_FUCHSIA_DEBUGDATA_H_
