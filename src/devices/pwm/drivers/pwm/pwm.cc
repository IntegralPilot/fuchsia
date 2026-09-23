// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "pwm.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/metadata/cpp/metadata.h>

#include <span>

#include <bind/fuchsia/cpp/bind.h>

#include "src/devices/pwm/drivers/pwm/pwm_parser.h"

namespace pwm {

namespace {

fuchsia_hardware_pwm::PwmChannelsMetadata ConvertMetadata(
    const pwm_metadata::PwmMetadata& generic) {
  std::vector<fuchsia_hardware_pwm::PwmChannelInfo> channels;
  for (const pwm_metadata::PwmChannelInfo& c : generic.channels) {
    fuchsia_hardware_pwm::PwmChannelInfo info;
    info.id(c.channel);
    if (c.id) {
      info.global_id(*c.id);
    }
    if (c.name) {
      info.name(*c.name);
    }
    info.period_ns(c.period_ns);
    channels.push_back(std::move(info));
  }
  return {{.channels = std::move(channels)}};
}

}  // namespace

zx::result<> Pwm::Start(fdf::DriverContext context) {
  std::optional<fuchsia_hardware_pwm::PwmChannelsMetadata> metadata;
  {
    // Try to get generic metadata first
    zx::result generic_res =
        fdf_metadata::GetMetadataFromFidlServiceIfExists<fuchsia_driver_metadata::Dictionary>(
            context.incoming().svc_dir(), "fuchsia.hardware.pwm.PwmChannelsMetadata");
    if (generic_res.is_ok() && generic_res.value().has_value()) {
      const std::optional parsed = pwm_metadata::PwmMetadata::Parse(*generic_res.value());
      if (parsed) {
        metadata = ConvertMetadata(*parsed);
      } else {
        fdf::error("Failed to parse generic PWM metadata");
      }
    }

    if (!metadata.has_value()) {
      // Fall back to old metadata
      zx::result metadata_res =
          fdf_metadata::GetMetadata<fuchsia_hardware_pwm::PwmChannelsMetadata>(context.incoming());
      if (metadata_res.is_error()) {
        fdf::error("Failed to get metadata: {}", metadata_res);
        return metadata_res.take_error();
      }
      metadata = std::move(*metadata_res);
    }
  }

  if (!metadata.value().channels().has_value()) {
    fdf::error("Metadata missing `channels` field");
    return zx::error(ZX_ERR_INTERNAL);
  }
  const std::span<const fuchsia_hardware_pwm::PwmChannelInfo> pwm_channels =
      metadata.value().channels().value();

  for (size_t i = 0; i < pwm_channels.size(); ++i) {
    const fuchsia_hardware_pwm::PwmChannelInfo& pwm_channel_info = pwm_channels[i];
    if (!pwm_channel_info.id().has_value()) {
      fdf::error("PWM channel info {} missing `id` field", i);
      return zx::error(ZX_ERR_INTERNAL);
    }
    const uint32_t pwm_channel_id = pwm_channel_info.id().value();

    zx::result pwm_impl = context.incoming().Connect<fuchsia_hardware_pwmimpl::Service::Device>();
    if (pwm_impl.is_error()) {
      fdf::error("Failed to connect to pwm-impl: {}", pwm_impl.status_string());
      return pwm_impl.take_error();
    }

    auto& pwm_channel = *pwm_channels_.emplace_back(std::make_unique<PwmChannel>(
        pwm_channel_id, pwm_channel_info.global_id(), pwm_channel_info.name(), dispatcher(),
        std::move(pwm_impl.value())));
    zx::result result = pwm_channel.Init(outgoing(), node());
    if (result.is_error()) {
      fdf::error("Failed to initialize pwm channel {}: {}", i, result);
      return result.take_error();
    }
  }

  return zx::ok();
}

zx::result<> PwmChannel::Init(std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
                              fidl::UnownedClientEnd<fuchsia_driver_framework::Node> parent) {
  const std::string child_node_name = std::format("pwm-{}", id_);

  {
    zx::result result = outgoing->AddService<fuchsia_hardware_pwm::Service>(
        fuchsia_hardware_pwm::Service::InstanceHandler{
            {.pwm = bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure)}},
        child_node_name);
    if (result.is_error()) {
      fdf::error("Failed to add pwm service to outgoing directory: {}", result);
      return result.take_error();
    }
  }

  zx::result connector = devfs_connector_.Bind(dispatcher_);
  if (connector.is_error()) {
    fdf::error("Failed to bind devfs connector: {}", connector);
    return connector.take_error();
  }

  fuchsia_driver_framework::DevfsAddArgs devfs_args({
      .connector = std::move(connector.value()),
      .class_name{kClassName},
      .connector_supports{fuchsia_device_fs::ConnectionType::kController},
  });

  const std::vector<fuchsia_driver_framework::Offer> offers = {
      fdf::MakeOffer2<fuchsia_hardware_pwm::Service>(child_node_name),
  };

  std::vector<fuchsia_driver_framework::NodeProperty2> properties = {
      fdf::MakeProperty2(bind_fuchsia::ID, id_),
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.pwm.Service"),
  };
  if (global_id_.has_value()) {
    properties.push_back(fdf::MakeProperty2(bind_fuchsia::ID, *global_id_));
  }
  if (name_.has_value()) {
    properties.push_back(fdf::MakeProperty2(bind_fuchsia::NAME, *name_));
  }

  zx::result child = fdf::AddChild(parent, *fdf::Logger::GlobalInstance(), child_node_name,
                                   devfs_args, properties, offers);
  if (child.is_error()) {
    fdf::error("Failed to add child: {}", child);
    return child.take_error();
  }
  child_ = std::move(child.value());

  return zx::ok();
}

void PwmChannel::GetConfig(GetConfigCompleter::Sync& completer) {
  fdf::Arena arena('PWMG');
  fdf::WireUnownedResult<fuchsia_hardware_pwmimpl::PwmImpl::GetConfig> result =
      pwm_impl_.buffer(arena)->GetConfig(id_);
  if (!result.ok()) {
    fdf::error("Failed to send GetConfig request: {}", result.status_string());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("Failed to get config: {}", result->error_value());
    completer.ReplyError(result->error_value());
    return;
  }

  completer.ReplySuccess(result->value()->config);
}

void PwmChannel::SetConfig(SetConfigRequestView request, SetConfigCompleter::Sync& completer) {
  fdf::Arena arena('PWMS');
  fdf::WireUnownedResult<fuchsia_hardware_pwmimpl::PwmImpl::SetConfig> result =
      pwm_impl_.buffer(arena)->SetConfig(id_, request->config);
  if (!result.ok()) {
    fdf::error("Failed to send SetConfig request: {}", result.status_string());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("Failed to set config: {}", result->error_value());
    completer.ReplyError(result->error_value());
    return;
  }

  completer.ReplySuccess();
}

void PwmChannel::Enable(EnableCompleter::Sync& completer) {
  fdf::Arena arena('PWME');
  fdf::WireUnownedResult<fuchsia_hardware_pwmimpl::PwmImpl::Enable> result =
      pwm_impl_.buffer(arena)->Enable(id_);
  if (!result.ok()) {
    fdf::error("Failed to send Enable request to pwm {}: {}", id_, result.status_string());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("Failed to enable pwm {}: {}", id_, zx_status_get_string(result->error_value()));
    completer.ReplyError(result->error_value());
    return;
  }

  completer.ReplySuccess();
}

void PwmChannel::Disable(DisableCompleter::Sync& completer) {
  fdf::Arena arena('PWMD');
  fdf::WireUnownedResult<fuchsia_hardware_pwmimpl::PwmImpl::Disable> result =
      pwm_impl_.buffer(arena)->Disable(id_);
  if (!result.ok()) {
    fdf::error("Failed to send Disable request to pwm {}: {}", id_, result.status_string());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("Failed to disable pwm {}: {}", id_, zx_status_get_string(result->error_value()));
    completer.ReplyError(result->error_value());
    return;
  }

  completer.ReplySuccess();
}

void PwmChannel::Connect(fidl::ServerEnd<fuchsia_hardware_pwm::Pwm> request) {
  bindings_.AddBinding(dispatcher_, std::move(request), this, fidl::kIgnoreBindingClosure);
}

}  // namespace pwm

FUCHSIA_DRIVER_EXPORT2(pwm::Pwm);
