// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/adc/drivers/adc/adc.h"

#include <fidl/fuchsia.hardware.adcimpl/cpp/driver/fidl.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/logging/cpp/structured_logger.h>

#include <bind/fuchsia/adc/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <fbl/alloc_checker.h>
#include <sdk/lib/driver/metadata/cpp/metadata.h>

#include "src/devices/adc/drivers/adc/adc_parser.h"

namespace adc {

AdcDevice::AdcDevice(fdf::ClientEnd<fuchsia_hardware_adcimpl::Device> adc_impl, uint32_t channel,
                     std::string_view name, std::optional<uint8_t> resolution, Adc* adc)
    : adc_impl_(std::move(adc_impl)),
      channel_(channel),
      name_(name),
      resolution_(resolution),
      devfs_connector_(fit::bind_member<&AdcDevice::Serve>(this)) {}

void AdcDevice::GetResolution(GetResolutionCompleter::Sync& completer) {
  if (!resolution_) {
    completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }
  completer.Reply(fit::ok(*resolution_));
}

void AdcDevice::GetSample(GetSampleCompleter::Sync& completer) {
  fdf::Arena arena('ADC_');
  const auto result = adc_impl_.buffer(arena)->GetSample(channel_);
  if (!result.ok()) {
    completer.Reply(fit::error(result.status()));
    return;
  }
  if (result->is_error()) {
    completer.Reply(fit::error(result->error_value()));
    return;
  }
  completer.Reply(fit::ok(result.value()->value));
}

void AdcDevice::GetNormalizedSample(GetNormalizedSampleCompleter::Sync& completer) {
  if (!resolution_) {
    completer.Reply(fit::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }

  fdf::Arena arena('ADC_');
  float sample;
  {
    const auto result = adc_impl_.buffer(arena)->GetSample(channel_);
    if (!result.ok()) {
      completer.Reply(fit::error(result.status()));
      return;
    }
    if (result->is_error()) {
      completer.Reply(fit::error(result->error_value()));
      return;
    }
    sample = static_cast<float>(result->value()->value);
  }

  completer.Reply(fit::ok(sample / static_cast<float>(((1 << *resolution_) - 1))));
}

zx::result<std::unique_ptr<AdcDevice>> AdcDevice::Create(
    fdf::ClientEnd<fuchsia_hardware_adcimpl::Device> adc_impl,
    fuchsia_hardware_adcimpl::AdcChannel channel, Adc* adc) {
  std::optional<uint8_t> resolution = std::nullopt;
  {
    fdf::Arena arena('ADC_');
    const auto result = fdf::WireCall(adc_impl).buffer(arena)->GetResolution();
    if (!result.ok()) {
      fdf::error("Failed to GetResolution {}", result.FormatDescription().c_str());
      return zx::error(ZX_ERR_INTERNAL);
    }
    if (result->is_error()) {
      fdf::warn("Failed to GetResolution {}", result->error_value());
    } else {
      resolution = result->value()->resolution;
    }
  }

  auto dev = std::make_unique<AdcDevice>(std::move(adc_impl), *channel.idx(),
                                         std::move(*channel.name()), resolution, adc);

  // Serve fuchsia_hardware_adc.
  {
    auto result = adc->outgoing()->AddService<fuchsia_hardware_adc::Service>(
        fuchsia_hardware_adc::Service::InstanceHandler({
            .device = dev->bindings_.CreateHandler(
                dev.get(), fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                fidl::kIgnoreBindingClosure),
        }),
        dev->name_);
    if (result.is_error()) {
      fdf::error("Failed to add Device service {}", result);
      return zx::error(result.status_value());
    }
  }

  // Create node.
  zx::result connector =
      dev->devfs_connector_.Bind(fdf::Dispatcher::GetCurrent()->async_dispatcher());
  if (connector.is_error()) {
    return connector.take_error();
  }

  auto offers = std::vector{fdf::MakeOffer2<fuchsia_hardware_adc::Service>(dev->name_)};
  auto properties = std::vector{
      fdf::MakeProperty2(bind_fuchsia_adc::CHANNEL, dev->channel_),
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.adc.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, dev->name_),
  };
  if (channel.id().has_value()) {
    properties.push_back(fdf::MakeProperty2(bind_fuchsia::ID, *channel.id()));
  }

  auto devfs_args = fuchsia_driver_framework::DevfsAddArgs{{
      .connector = std::move(connector.value()),
      .class_name = "adc",
  }};

  auto result = adc->AddChild(dev->name_, devfs_args, properties, offers);
  if (result.is_error()) {
    fdf::error("Failed to add child {}", result);
    return zx::error(result.status_value());
  }
  dev->controller_.Bind(std::move(result.value()));

  return zx::ok(std::move(dev));
}

namespace {
fuchsia_hardware_adcimpl::Metadata ConvertMetadata(const adc_metadata::AdcMetadata& generic) {
  std::vector<fuchsia_hardware_adcimpl::AdcChannel> channels;
  for (const auto& c : generic.channels) {
    fuchsia_hardware_adcimpl::AdcChannel channel;
    channel.idx(c.channel);
    if (c.id) {
      channel.id(*c.id);
    }
    if (c.name) {
      channel.name(*c.name);
    }
    channels.push_back(std::move(channel));
  }
  return fuchsia_hardware_adcimpl::Metadata{{.channels = std::move(channels)}};
}
}  // namespace

zx::result<> Adc::Start(fdf::DriverContext context) {
  // Get metadata.
  std::optional<fuchsia_hardware_adcimpl::Metadata> metadata;
  {
    // Try to get generic metadata first
    zx::result generic_res =
        fdf_metadata::GetMetadataFromFidlServiceIfExists<fuchsia_driver_metadata::Dictionary>(
            context.svc(), "fuchsia.hardware.adcimpl.Metadata");
    if (generic_res.is_ok() && generic_res.value().has_value()) {
      auto parsed = adc_metadata::AdcMetadata::Parse(*generic_res.value());
      if (parsed) {
        metadata = ConvertMetadata(*parsed);
      } else {
        fdf::error("Failed to parse generic ADC metadata");
      }
    }

    if (!metadata.has_value()) {
      // Fall back to old metadata
      zx::result metadata_res =
          fdf_metadata::GetMetadata<fuchsia_hardware_adcimpl::Metadata>(context.incoming());
      if (metadata_res.is_error()) {
        fdf::error("Failed to get metadata: {}", metadata_res.status_string());
        return metadata_res.take_error();
      }
      metadata = std::move(*metadata_res);
    }
  }
  if (!metadata->channels().has_value()) {
    fdf::error("Metadata is missing its channels property");
    return zx::error(ZX_ERR_INTERNAL);
  }
  auto channels = std::move(*metadata->channels());

  // Make sure that the list of ADC channels has no duplicates.
  auto adc_cmp_lt = [](fuchsia_hardware_adcimpl::AdcChannel& lhs,
                       fuchsia_hardware_adcimpl::AdcChannel& rhs) {
    return *lhs.idx() < *rhs.idx();
  };
  auto adc_cmp_eq = [](fuchsia_hardware_adcimpl::AdcChannel& lhs,
                       fuchsia_hardware_adcimpl::AdcChannel& rhs) {
    return *lhs.idx() == *rhs.idx();
  };
  std::sort(channels.begin(), channels.end(), adc_cmp_lt);
  auto result = std::adjacent_find(channels.begin(), channels.end(), adc_cmp_eq);
  if (result != channels.end()) {
    fdf::error("adc channel '{}' was published more than once", *result->idx());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Create a device per channel.
  for (auto channel : channels) {
    auto adcimpl = context.incoming().Connect<fuchsia_hardware_adcimpl::Service::Device>();
    if (adcimpl.is_error()) {
      fdf::error("Failed to open adcimpl service: {}", adcimpl);
      return adcimpl.take_error();
    }

    auto device = AdcDevice::Create(std::move(adcimpl.value()), std::move(channel), this);
    if (device.is_error()) {
      fdf::error("Failed to add create device {}", device.status_value());
      return zx::error(device.status_value());
    }
    devices_.emplace_back(std::move(*device));
  }

  return zx::ok();
}

void Adc::Stop(fdf::StopCompleter completer) {
  for (auto& dev : devices_) {
    auto result = dev->controller()->Remove();
    if (!result.ok()) {
      fdf::error("Could not remove child: {}", result.status_string());
    }
  }
  completer(zx::ok());
}

}  // namespace adc

FUCHSIA_DRIVER_EXPORT2(adc::Adc);
