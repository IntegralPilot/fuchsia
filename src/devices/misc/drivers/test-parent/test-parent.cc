// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/test/cpp/bind.h>

namespace {

class TestParent : public fdf::DriverBase2 {
 public:
  static constexpr std::string_view kDriverName = "test-parent";
  static constexpr std::string_view kChildNodeName = "sys";
  static constexpr std::string_view kGrandchildNodeName = "test";

  TestParent() : DriverBase2(kDriverName) {}

  zx::result<> Start(fdf::DriverContext context) override;

 private:
  fdf::OwnedChildNode child_;
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> grandchild_;
  compat::SyncInitializedDeviceServer compat_server_;
};

zx::result<> TestParent::Start(fdf::DriverContext context) {
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx::result<> result =
      compat_server_.Initialize(incoming, outgoing(), context.node_name(), kGrandchildNodeName,
                                compat::ForwardMetadata::None());
  if (result.is_error()) {
    fdf::error("Failed to initialize compat server: {}", result);
    return result.take_error();
  }

  // Add child.
  zx::result child = AddOwnedChild(kChildNodeName);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  // Add grandchild.
  std::vector properties = {
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL, bind_fuchsia_test::BIND_PROTOCOL_PARENT),
  };
  std::vector offers = compat_server_.CreateOffers2();
  zx::result grandchild =
      fdf::AddChild(child_.node_, logger(), kGrandchildNodeName, properties, offers);
  if (grandchild.is_error()) {
    fdf::error("Failed to add grandchild: {}", grandchild);
    return grandchild.take_error();
  }
  grandchild_ = std::move(grandchild.value());

  return zx::ok();
}

}  // namespace

FUCHSIA_DRIVER_EXPORT2(TestParent);
