// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/driver/metadata/cpp/tests/metadata_integration_test/metadata_forwarder_test_driver/metadata_forwarder_test_driver.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/structured_logger.h>

#include <bind/fuchsia_driver_metadata_test_bind_library/cpp/bind.h>

namespace fdf_metadata::test {

zx::result<> MetadataForwarderTestDriver::Start(fdf::DriverContext context) {
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx::result result = metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), incoming);
  if (result.is_error()) {
    fdf::error("Failed to forward and serve metadata: {}", result);
    return result.take_error();
  }
#else
  fdf::error("Forwarding metadata not supported at current Fuchsia API level.");
  return zx::error(ZX_ERR_NOT_SUPPORTED);
#endif

  static const std::vector<fuchsia_driver_framework::NodeProperty2> kNodeProperties{
      fdf::MakeProperty2(bind_fuchsia_driver_metadata_test::PURPOSE,
                         bind_fuchsia_driver_metadata_test::PURPOSE_RETRIEVE_METADATA),
      fdf::MakeProperty2(bind_fuchsia_driver_metadata_test::USES_METADATA_FIDL_SERVICE, true)};

  std::vector<fuchsia_driver_framework::Offer> offers;
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  std::optional metadata_offer = metadata_server_.CreateOffer();
  if (metadata_offer.has_value()) {
    offers.push_back(metadata_offer.value());
  }
#endif
  zx::result child = AddChild(kChildNodeName, kNodeProperties, std::move(offers));
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  return zx::ok();
}

}  // namespace fdf_metadata::test

FUCHSIA_DRIVER_EXPORT2(fdf_metadata::test::MetadataForwarderTestDriver);
