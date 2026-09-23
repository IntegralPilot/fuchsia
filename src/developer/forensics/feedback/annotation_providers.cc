// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/annotation_providers.h"

#include <memory>
#include <utility>

#include "src/developer/forensics/feedback/annotations/battery_info_provider.h"
#include "src/developer/forensics/feedback/annotations/constants.h"
#include "src/developer/forensics/feedback/constants.h"
#include "src/lib/backoff/exponential_backoff.h"
#include "src/lib/timekeeper/system_clock.h"

namespace forensics::feedback {

namespace {

bool IncludesAnyInAllowlist(const std::set<std::string>& keys,
                            const std::set<std::string>& allowlist) {
  return std::any_of(keys.begin(), keys.end(),
                     [&allowlist](const std::string& key) { return allowlist.contains(key); });
}

}  // namespace

AnnotationProviders::AnnotationProviders(
    async_dispatcher_t* dispatcher, std::shared_ptr<sys::ServiceDirectory> services,
    std::set<std::string> allowlist, const std::set<std::string>& product_exclude_list,
    Annotations static_annotations,
    std::unique_ptr<CachedAsyncAnnotationProvider> device_id_provider)
    : dispatcher_(dispatcher),
      data_register_(kMaxNumNonPlatformAnnotations, kReservedAnnotationNamespaces,
                     kDataRegisterPath),
      device_id_provider_(std::move(device_id_provider)),
      annotation_manager_(dispatcher_, allowlist, static_annotations, &data_register_,
                          GetDynamicSyncProviders(services, allowlist),
                          GetStaticAsyncProviders(services, allowlist),
                          GetCachedAsyncProviders(services, allowlist),
                          GetDynamicAsyncProviders(services, allowlist), product_exclude_list) {
  FX_CHECK(allowlist.size() <= kMaxNumPlatformAnnotations)
      << "Requesting " << allowlist.size() << " annotations when " << kMaxNumPlatformAnnotations
      << " are alloted for the platform";

  if (allowlist.empty()) {
    FX_LOGS(WARNING) << "Annotation allowlist is empty, no platform annotations will be collected";
  }
}

fuchsia::feedback::ComponentDataRegister* AnnotationProviders::ComponentDataRegister() {
  return &data_register_;
}

std::unique_ptr<backoff::Backoff> AnnotationProviders::AnnotationProviderBackoff() {
  return std::unique_ptr<backoff::Backoff>(
      new backoff::ExponentialBackoff(zx::min(1), 2u, zx::hour(1)));
}

std::vector<DynamicSyncAnnotationProvider*> AnnotationProviders::GetDynamicSyncProviders(
    const std::shared_ptr<sys::ServiceDirectory>& services,
    const std::set<std::string>& allowlist) {
  std::vector<DynamicSyncAnnotationProvider*> providers;

  if (IncludesAnyInAllowlist(TimeProvider::GetAnnotationKeys(), allowlist)) {
    time_provider_ =
        std::make_unique<TimeProvider>(dispatcher_, zx::unowned_clock(zx_utc_reference_get()),
                                       std::make_unique<timekeeper::SystemClock>());
    providers.push_back(time_provider_.get());
  }

  if (IncludesAnyInAllowlist(UIStateProvider::GetAnnotationKeys(), allowlist)) {
    providers.push_back(GetOrConstructUIStateProvider(services));
  }

  return providers;
}

std::vector<StaticAsyncAnnotationProvider*> AnnotationProviders::GetStaticAsyncProviders(
    const std::shared_ptr<sys::ServiceDirectory>& services,
    const std::set<std::string>& allowlist) {
  std::vector<StaticAsyncAnnotationProvider*> providers;

  if (IncludesAnyInAllowlist(BoardInfoProvider::GetAnnotationKeys(), allowlist)) {
    board_info_provider_ =
        std::make_unique<BoardInfoProvider>(dispatcher_, services, AnnotationProviderBackoff());
    providers.push_back(board_info_provider_.get());
  }

  if (IncludesAnyInAllowlist(ProductInfoProvider::GetAnnotationKeys(), allowlist)) {
    product_info_provider_ =
        std::make_unique<ProductInfoProvider>(dispatcher_, services, AnnotationProviderBackoff());
    providers.push_back(product_info_provider_.get());
  }

  if (IncludesAnyInAllowlist(CurrentChannelProvider::GetAnnotationKeys(), allowlist)) {
    current_channel_provider_ = std::make_unique<CurrentChannelProvider>(
        dispatcher_, services, AnnotationProviderBackoff());
    providers.push_back(current_channel_provider_.get());
  }

  return providers;
}

std::vector<CachedAsyncAnnotationProvider*> AnnotationProviders::GetCachedAsyncProviders(
    const std::shared_ptr<sys::ServiceDirectory>& services,
    const std::set<std::string>& allowlist) {
  std::vector<CachedAsyncAnnotationProvider*> providers;
  providers.push_back(device_id_provider_.get());

  if (IncludesAnyInAllowlist(IntlProvider::GetAnnotationKeys(), allowlist)) {
    intl_provider_ =
        std::make_unique<IntlProvider>(dispatcher_, services, AnnotationProviderBackoff());
    providers.push_back(intl_provider_.get());
  }

  if (IncludesAnyInAllowlist(UIStateProvider::GetAnnotationKeys(), allowlist)) {
    providers.push_back(GetOrConstructUIStateProvider(services));
  }

  return providers;
}

std::vector<DynamicAsyncAnnotationProvider*> AnnotationProviders::GetDynamicAsyncProviders(
    const std::shared_ptr<sys::ServiceDirectory>& services,
    const std::set<std::string>& allowlist) {
  std::vector<DynamicAsyncAnnotationProvider*> providers;

  if (IncludesAnyInAllowlist(TargetChannelProvider::GetAnnotationKeys(), allowlist)) {
    target_channel_provider_ =
        std::make_unique<TargetChannelProvider>(dispatcher_, services, AnnotationProviderBackoff());
    providers.push_back(target_channel_provider_.get());
  }

  if (IncludesAnyInAllowlist(BatteryInfoProvider::GetAnnotationKeys(), allowlist)) {
    battery_info_provider_ =
        std::make_unique<BatteryInfoProvider>(dispatcher_, services, AnnotationProviderBackoff());
    providers.push_back(battery_info_provider_.get());
  }

  return providers;
}

UIStateProvider* AnnotationProviders::GetOrConstructUIStateProvider(
    const std::shared_ptr<sys::ServiceDirectory>& services) {
  if (ui_state_provider_ == nullptr) {
    ui_state_provider_ = std::make_unique<UIStateProvider>(
        dispatcher_, services, std::make_unique<timekeeper::SystemClock>(),
        AnnotationProviderBackoff());
  }

  return ui_state_provider_.get();
}

}  // namespace forensics::feedback
