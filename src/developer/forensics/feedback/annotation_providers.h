// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATION_PROVIDERS_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATION_PROVIDERS_H_

#include <fuchsia/feedback/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/fidl/cpp/interface_request.h>
#include <lib/sys/cpp/service_directory.h>

#include <memory>

#include "src/developer/forensics/feedback/annotations/annotation_manager.h"
#include "src/developer/forensics/feedback/annotations/battery_info_provider.h"
#include "src/developer/forensics/feedback/annotations/board_info_provider.h"
#include "src/developer/forensics/feedback/annotations/current_channel_provider.h"
#include "src/developer/forensics/feedback/annotations/data_register.h"
#include "src/developer/forensics/feedback/annotations/intl_provider.h"
#include "src/developer/forensics/feedback/annotations/product_info_provider.h"
#include "src/developer/forensics/feedback/annotations/provider.h"
#include "src/developer/forensics/feedback/annotations/target_channel_provider.h"
#include "src/developer/forensics/feedback/annotations/time_provider.h"
#include "src/developer/forensics/feedback/annotations/ui_state_provider.h"
#include "src/lib/backoff/backoff.h"

namespace forensics::feedback {

// Wraps the annotations providers Feedback uses and the component's AnnotationManager.
class AnnotationProviders {
 public:
  AnnotationProviders(async_dispatcher_t* dispatcher,
                      std::shared_ptr<sys::ServiceDirectory> services,
                      std::set<std::string> allowlist,
                      const std::set<std::string>& product_exclude_list,
                      Annotations static_annotations,
                      std::unique_ptr<CachedAsyncAnnotationProvider> device_id_provider);

  AnnotationManager* GetAnnotationManager() { return &annotation_manager_; }

  fuchsia::feedback::ComponentDataRegister* ComponentDataRegister();

  static std::unique_ptr<backoff::Backoff> AnnotationProviderBackoff();

 private:
  std::vector<DynamicSyncAnnotationProvider*> GetDynamicSyncProviders(
      const std::shared_ptr<sys::ServiceDirectory>& services,
      const std::set<std::string>& allowlist);

  std::vector<StaticAsyncAnnotationProvider*> GetStaticAsyncProviders(
      const std::shared_ptr<sys::ServiceDirectory>& services,
      const std::set<std::string>& allowlist);

  std::vector<CachedAsyncAnnotationProvider*> GetCachedAsyncProviders(
      const std::shared_ptr<sys::ServiceDirectory>& services,
      const std::set<std::string>& allowlist);

  std::vector<DynamicAsyncAnnotationProvider*> GetDynamicAsyncProviders(
      const std::shared_ptr<sys::ServiceDirectory>& services,
      const std::set<std::string>& allowlist);

  UIStateProvider* GetOrConstructUIStateProvider(
      const std::shared_ptr<sys::ServiceDirectory>& services);

  async_dispatcher_t* dispatcher_;

  DataRegister data_register_;
  std::unique_ptr<CachedAsyncAnnotationProvider> device_id_provider_;

  // These providers may not be constructed if they're not needed.
  std::unique_ptr<TimeProvider> time_provider_;
  std::unique_ptr<BoardInfoProvider> board_info_provider_;
  std::unique_ptr<ProductInfoProvider> product_info_provider_;
  std::unique_ptr<CurrentChannelProvider> current_channel_provider_;
  std::unique_ptr<IntlProvider> intl_provider_;
  std::unique_ptr<TargetChannelProvider> target_channel_provider_;
  std::unique_ptr<BatteryInfoProvider> battery_info_provider_;
  std::unique_ptr<UIStateProvider> ui_state_provider_;

  AnnotationManager annotation_manager_;
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATION_PROVIDERS_H_
