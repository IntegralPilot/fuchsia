// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_POWER_CPP_TESTING_FIDL_TEST_BASE_DEFAULT_H_
#define LIB_DRIVER_POWER_CPP_TESTING_FIDL_TEST_BASE_DEFAULT_H_

#include <lib/fidl/cpp/unified_messaging_declarations.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>

namespace fdf_power::testing {

template <typename Protocol>
class FidlTestBaseDefault : public fidl::testing::TestBase<Protocol> {
 private:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) final {
    ZX_PANIC("Unexpected call: %s", name.c_str());
  }
  void handle_unknown_method(fidl::UnknownMethodMetadata<Protocol> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) final {
    ZX_PANIC("Encountered unknown method");
  }
};

}  // namespace fdf_power::testing

#endif  // LIB_DRIVER_POWER_CPP_TESTING_FIDL_TEST_BASE_DEFAULT_H_
