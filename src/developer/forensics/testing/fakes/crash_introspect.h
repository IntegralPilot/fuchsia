// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_TESTING_FAKES_CRASH_INTROSPECT_H_
#define SRC_DEVELOPER_FORENSICS_TESTING_FAKES_CRASH_INTROSPECT_H_

#include <fidl/fuchsia.driver.crash/cpp/fidl.h>
#include <fidl/fuchsia.sys2/cpp/fidl.h>

namespace forensics::fakes {

class CrashIntrospect : public fidl::Server<fuchsia_sys2::CrashIntrospect>,
                        public fidl::Server<fuchsia_driver_crash::CrashIntrospect> {
 public:
  void FindComponentByThreadKoid(FindComponentByThreadKoidRequest& request,
                                 FindComponentByThreadKoidCompleter::Sync& completer) override;
  void FindDriverCrash(FindDriverCrashRequest& request,
                       FindDriverCrashCompleter::Sync& completer) override;
};

}  // namespace forensics::fakes

#endif  // SRC_DEVELOPER_FORENSICS_TESTING_FAKES_CRASH_INTROSPECT_H_
