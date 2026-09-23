// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/driver/devicetree/visitors/drivers/mailbox-controllers/mailbox/mailbox-visitor.h"

#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/common-types.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

// TODO(https://fxbug.dev/494450198: Re-add this once the Bazel dependency issue is resoled.
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/mailbox/cpp/bind.h>

namespace {

// TODO(https://fxbug.dev/494450198): Remove this once we fix the Bazel dependency issue for FIDL
// generated bind cpp headers

constexpr char kMailboxesProperty[] = "mboxes";
constexpr char kMailboxNamesProperty[] = "mbox-names";
constexpr char kMailboxCellsProperty[] = "#mbox-cells";

struct MailboxSpec {
  uint32_t channel;
  std::optional<uint32_t> client;
};

zx::result<MailboxSpec> ParseMailbox(fdf_devicetree::Reference& reference) {
  auto cells = reference.property_cells();
  if (cells.size_bytes() == sizeof(uint32_t)) {
    const std::optional<uint32_t> channel = devicetree::PropertyValue(cells).AsUint32();
    if (!channel) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    return zx::ok(MailboxSpec{.channel = *channel});
  }

  if (cells.size_bytes() == 2 * sizeof(uint32_t)) {
    using MailboxElement = devicetree::PropEncodedArrayElement<2>;
    devicetree::PropEncodedArray<MailboxElement> mailbox_cells(cells, 1, 1);
    return zx::ok(MailboxSpec{
        .channel = static_cast<uint32_t>(*mailbox_cells[0][0]),
        .client = static_cast<uint32_t>(*mailbox_cells[0][1]),
    });
  }
  return zx::error(ZX_ERR_INVALID_ARGS);
}

}  // namespace

namespace mailbox_dt {

MailboxVisitor::MailboxVisitor() {
  fdf_devicetree::Properties mailbox_properties = {};
  mailbox_properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kMailboxesProperty, kMailboxCellsProperty, /*required=*/false));
  mailbox_parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(mailbox_properties));
}

zx::result<> MailboxVisitor::Visit(fdf_devicetree::Node& node,
                                   const devicetree::PropertyDecoder& decoder) {
  auto properties = mailbox_parser_->Parse(node);
  if (properties.is_error()) {
    fdf::error("Failed to parse node \"{}\"", node.name());

    return properties.take_error();
  }
  auto channels = properties->Get<fdf_devicetree::References>(kMailboxesProperty);
  if (!channels) {
    return zx::ok();
  }

  std::vector<std::string_view> channel_names;
  auto channel_names_property = node.GetProperty<std::vector<std::string>>(kMailboxNamesProperty);
  if (channel_names_property.is_ok()) {
    channel_names.reserve(channel_names_property->size());
    for (const auto& name : *channel_names_property) {
      channel_names.push_back(name);
    }
  } else if (channel_names_property.status_value() != ZX_ERR_NOT_FOUND) {
    fdf::error("Failed to parse mbox-names property for node \"{}\": {}", node.name(),
               channel_names_property);

    return channel_names_property.take_error();
  }

  if (!channel_names.empty() && channels->size() != channel_names.size()) {
    fdf::error("mboxes and mbox-names mismatch for node \"{}\"", node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto channel_name_it = channel_names.cbegin();

  for (auto& reference : *channels) {
    zx::result<MailboxSpec> spec = ParseMailbox(reference);
    if (spec.is_error()) {
      fdf::error("Failed to parse mailbox channel reference for node \"{}\"", node.name());

      return spec.take_error();
    }

    fuchsia_hardware_mailbox::ChannelInfo channel_info;
    channel_info.channel(spec->channel);
    if (spec->client) {
      channel_info.client(*spec->client);
    }
    controller_info_[reference.reference_node().id()].push_back(std::move(channel_info));

    fuchsia_driver_framework::ParentSpec2 parent_spec{{
        .bind_rules =
            {
                fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.mailbox.Service"),
                fdf::MakeAcceptBindRule(bind_fuchsia_mailbox::CONTROLLER_ID,
                                        reference.reference_node().id()),
                fdf::MakeAcceptBindRule(bind_fuchsia::ID, spec->channel),
            },
        .properties =
            {
                fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.mailbox.Service"),
                fdf::MakeProperty2(bind_fuchsia::ID, spec->channel),
                fdf::MakeProperty2(bind_fuchsia::NAME, channel_name_it != channel_names.cend()
                                                           ? *channel_name_it
                                                           : "mailbox"),
            },
    }};

    if (spec->client) {
      parent_spec.bind_rules().push_back(
          fdf::MakeAcceptBindRule(bind_fuchsia_mailbox::CLIENT, *spec->client));
    }

    if (channel_name_it != channel_names.cend()) {
      ++channel_name_it;
    }

    node.AddNodeSpec(parent_spec);
  }

  return zx::ok();
}

zx::result<> MailboxVisitor::FinalizeNode(fdf_devicetree::Node& node) {
  const uint32_t controller_id = node.id();

  const auto channels = controller_info_.find(controller_id);
  if (channels == controller_info_.end()) {
    return zx::ok();  // Not a mailbox controller or no channels -- ignore.
  }

  auto mbox_cells = node.GetProperty<uint32_t>(kMailboxCellsProperty);
  if (mbox_cells.is_error()) {
    fdf::error("Missing #mbox-cells property for node \"{}\": {}", node.name(), mbox_cells);

    return mbox_cells.take_error();
  }

  if (*mbox_cells != 1 && *mbox_cells != 2) {
    fdf::error("Invalid #mbox-cells property for node \"{}\"", node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fuchsia_hardware_mailbox::ControllerInfo controller{{.id = controller_id}};
  if (channels != controller_info_.end()) {
    controller.channels() = channels->second;
  }

  fit::result<fidl::Error, std::vector<uint8_t>> metadata = fidl::Persist(controller);
  if (metadata.is_error()) {
    fdf::error("Failed to persist mailbox controller metadata: {}",
               metadata.error_value().FormatDescription());

    return zx::error(metadata.error_value().status());
  }

  fuchsia_hardware_platform_bus::Metadata pbus_metadata{{
      .id = fuchsia_hardware_mailbox::ControllerInfo::kSerializableName,
      .data = *std::move(metadata),
  }};
  node.AddMetadata(std::move(pbus_metadata));

  return zx::ok();
}

}  // namespace mailbox_dt
