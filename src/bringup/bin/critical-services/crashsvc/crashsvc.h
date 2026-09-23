// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_BRINGUP_BIN_CRITICAL_SERVICES_CRASHSVC_CRASHSVC_H_
#define SRC_BRINGUP_BIN_CRITICAL_SERVICES_CRASHSVC_CRASHSVC_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async/cpp/wait.h>
#include <lib/zx/job.h>

// Initialize the crash service, this supersedes the standalone service with
// the same name that lived in zircon/system/core/crashsvc/crashsvc.cpp
// (/boot/bin/crashsvc) and ad-hoc microservice in devmgr that delegated to
// svchost. See https://fxbug.dev/42108157 for details.
//
// The job of this service is to handle exceptions that reached |root_job| and
// delegate the crash analysis to one of two services:
//
// - built-in : using system/ulib/inspector
// - remotely hosted: via FIDL interface call (fuchsia_exception_Handler).
//
// Which one depends if |exception_handler_svc| is a valid channel handle, which
// svchost sets depending on "use_system".
zx::result<std::unique_ptr<async::Wait>> start_crashsvc(
    async_dispatcher_t* dispatcher, zx::channel exception_channel,
    fidl::ClientEnd<fuchsia_io::Directory> exception_handler_svc);

#endif  // SRC_BRINGUP_BIN_CRITICAL_SERVICES_CRASHSVC_CRASHSVC_H_
