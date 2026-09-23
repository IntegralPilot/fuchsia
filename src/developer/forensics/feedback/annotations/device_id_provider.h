// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_DEVICE_ID_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_DEVICE_ID_PROVIDER_H_

#include <fidl/fuchsia.feedback/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/client.h>
#include <lib/fit/function.h>
#include <lib/sys/cpp/service_directory.h>

#include <memory>
#include <optional>
#include <set>
#include <string>

#include "src/developer/forensics/feedback/annotations/provider.h"
#include "src/developer/forensics/feedback/annotations/types.h"
#include "src/lib/backoff/backoff.h"

namespace forensics::feedback {

struct DeviceIdToAnnotations {
  Annotations operator()(const std::string& device_id);
};

// Fetches the device id from the file at |path|.
class LocalDeviceIdProvider : public CachedAsyncAnnotationProvider {
 public:
  explicit LocalDeviceIdProvider(const std::string& path);

  void GetOnUpdate(::fit::function<void(Annotations)> callback) override;

  std::set<std::string> GetKeys() const override;

 private:
  std::string device_id_;
};

// Fetches the device id from a FIDL server.
class RemoteDeviceIdProvider : public CachedAsyncAnnotationProvider,
                               public fidl::AsyncEventHandler<fuchsia_feedback::DeviceIdProvider> {
 public:
  RemoteDeviceIdProvider(async_dispatcher_t* dispatcher,
                         std::shared_ptr<sys::ServiceDirectory> services,
                         std::unique_ptr<backoff::Backoff> backoff);

  // |fidl::AsyncEventHandler<fuchsia_feedback::DeviceIdProvider>|
  void on_fidl_error(fidl::UnbindInfo info) override;

  // |CachedAsyncAnnotationProvider|
  void GetOnUpdate(::fit::function<void(Annotations)> callback) override;

  std::set<std::string> GetKeys() const override;

 private:
  bool Connect();
  void Call();

  async_dispatcher_t* dispatcher_;
  std::shared_ptr<sys::ServiceDirectory> services_;
  std::unique_ptr<backoff::Backoff> backoff_;

  fidl::Client<fuchsia_feedback::DeviceIdProvider> client_;
  std::optional<Annotations> last_annotations_;
  ::fit::function<void(Annotations)> on_update_;
  async::TaskClosureMethod<RemoteDeviceIdProvider, &RemoteDeviceIdProvider::Call> reconnect_task_{
      this};
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_DEVICE_ID_PROVIDER_H_
