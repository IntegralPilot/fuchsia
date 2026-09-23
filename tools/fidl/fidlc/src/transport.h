// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_TRANSPORT_H_
#define TOOLS_FIDL_FIDLC_SRC_TRANSPORT_H_

#include <optional>
#include <set>
#include <vector>

#include "tools/fidl/fidlc/src/name.h"

namespace fidlc {

// The class / namespace of the handle, used for compatibility checking with
// transports.
enum class HandleClass : uint8_t {
  kZircon,  // zx.Handle
  kDriver,  // fdf.handle
  kBanjo,   // only referenced by client_end / server_end
};

std::optional<HandleClass> HandleClassFromName(const Name& name);

struct Transport {
  enum class Kind : uint8_t {
    kZirconChannel,  // @transport("Channel")
    kDriverChannel,  // @transport("Driver")
    kBanjo,          // @transport("Banjo")
    kSyscall,        // @transport("Syscall")
  };

  // e.g. kZirconChannel.
  Kind kind;
  // e.g. "Channel".
  std::string_view name;
  // The class of handle used to represent client and server endpoints of this transport
  // (e.g. zx.Handle for @transport("Channel")).
  std::optional<HandleClass> handle_class;
  // The classes of handles that can be used in this transport.
  std::set<HandleClass> compatible_handle_classes;

  bool IsCompatible(HandleClass) const;
  static const Transport* FromTransportName(std::string_view transport_name);
  static std::set<std::string_view> AllTransportNames();

 private:
  static std::vector<Transport> transports;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_TRANSPORT_H_
