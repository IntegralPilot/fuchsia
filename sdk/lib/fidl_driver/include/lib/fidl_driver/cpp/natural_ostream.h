// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_DRIVER_CPP_NATURAL_OSTREAM_H_
#define LIB_FIDL_DRIVER_CPP_NATURAL_OSTREAM_H_

#include <lib/fidl/cpp/natural_ostream.h>
#include <lib/fidl_driver/cpp/natural_types.h>

namespace fidl::ostream {

template <>
struct Formatter<fdf::Channel> {
  static std::ostream& Format(std::ostream& os, const fdf::Channel& channel) {
    return os << "fdf::Channel(" << channel.get() << ")";
  }
};

}  // namespace fidl::ostream

#endif  // LIB_FIDL_DRIVER_CPP_NATURAL_OSTREAM_H_
