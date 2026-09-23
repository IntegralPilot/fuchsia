// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_INTL_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_INTL_PROVIDER_H_

#include <fidl/fuchsia.intl/cpp/fidl.h>
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
#include "src/developer/forensics/utils/errors.h"
#include "src/lib/backoff/backoff.h"

namespace forensics::feedback {

// Caches the most up-to-date version of the system locale and timezone.
//
// fuchsia.intl.PropertyProvider must be in |services|.
class IntlProvider : public CachedAsyncAnnotationProvider,
                     public fidl::AsyncEventHandler<fuchsia_intl::PropertyProvider> {
 public:
  IntlProvider(async_dispatcher_t* dispatcher, std::shared_ptr<sys::ServiceDirectory> services,
               std::unique_ptr<backoff::Backoff> backoff);

  static std::set<std::string> GetAnnotationKeys();
  std::set<std::string> GetKeys() const override;

  void GetOnUpdate(::fit::function<void(Annotations)> callback) override;

  // fidl::AsyncEventHandler<fuchsia_intl::PropertyProvider>
  void OnChange() override;
  void on_fidl_error(fidl::UnbindInfo info) override;

 private:
  bool Connect();
  void GetInternationalization();
  void OnUpdate();

  async_dispatcher_t* dispatcher_;
  const std::shared_ptr<sys::ServiceDirectory> services_;

  std::optional<std::string> locale_;
  std::optional<std::string> timezone_;
  std::optional<Error> error_;

  fidl::Client<fuchsia_intl::PropertyProvider> client_;
  std::unique_ptr<backoff::Backoff> backoff_;

  ::fit::function<void(Annotations)> on_update_;

  async::TaskClosureMethod<IntlProvider, &IntlProvider::GetInternationalization> reconnect_task_{
      this};
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_INTL_PROVIDER_H_
