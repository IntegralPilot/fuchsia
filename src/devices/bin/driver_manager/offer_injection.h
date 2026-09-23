// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_OFFER_INJECTION_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_OFFER_INJECTION_H_

#include <fidl/fuchsia.component.decl/cpp/fidl.h>

namespace driver_manager {

struct PowerOffersConfig {
  bool power_inject_offer;
  bool power_suspend_enabled;
};

// TODO(https://fxbug.dev/369189827): Once we can route these statically again, remove them as
// dynamic offers here.
class OfferInjector {
 public:
  explicit OfferInjector(PowerOffersConfig power_config) : power_config_(power_config) {}
  size_t ExtraOffersCount() const;
  void Inject(fidl::AnyArena& arena,
              fidl::VectorView<fuchsia_component_decl::wire::Offer>& dynamic_offers,
              size_t start_index) const;

 private:
  PowerOffersConfig power_config_;
};
}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_OFFER_INJECTION_H_
