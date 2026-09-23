// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.amlogic.metadata/cpp/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/google/platform/cpp/bind.h>
#include <bind/fuchsia/power/cpp/bind.h>
#include <soc/aml-s905d2/s905d2-pwm.h>

#include "astro-gpios.h"
#include "astro.h"

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace astro {
namespace fpbus = fuchsia_hardware_platform_bus;

zx_status_t AddPowerImpl(fdf::WireSyncClient<fuchsia_hardware_platform_bus::PlatformBus>& pbus) {
  static const fuchsia_hardware_amlogic_metadata::PowerMetadata kAmlogicMetadata(
      {.voltage_table =
           std::vector<fuchsia_hardware_amlogic_metadata::VoltageTableEntry>{
               {1'022'000, 0}, {1'011'000, 3}, {1'001'000, 6}, {991'000, 10}, {981'000, 13},
               {971'000, 16},  {961'000, 20},  {951'000, 23},  {941'000, 26}, {931'000, 30},
               {921'000, 33},  {911'000, 36},  {901'000, 40},  {891'000, 43}, {881'000, 46},
               {871'000, 50},  {861'000, 53},  {851'000, 56},  {841'000, 60}, {831'000, 63},
               {821'000, 67},  {811'000, 70},  {801'000, 73},  {791'000, 76}, {781'000, 80},
               {771'000, 83},  {761'000, 86},  {751'000, 90},  {741'000, 93}, {731'000, 96},
               {721'000, 100},
           },
       .voltage_pwm_period = zx::nsec(1250).get()});

  static const fuchsia_hardware_power::DomainMetadata kDomainMetadata(
      {.domains = {{{{.id = {bind_fuchsia_amlogic_platform::POWER_DOMAIN_ARM_CORE_BIG}}}}}});

  fit::result persisted_amlogic_metadata = fidl::Persist(kAmlogicMetadata);
  if (!persisted_amlogic_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist amlogic metadata: %s",
           persisted_amlogic_metadata.error_value().FormatDescription().c_str());
    return persisted_amlogic_metadata.error_value().status();
  }

  fit::result persisted_domain_metadata = fidl::Persist(kDomainMetadata);
  if (!persisted_domain_metadata.is_ok()) {
    zxlogf(ERROR, "Failed to persist power domain metadata: %s",
           persisted_domain_metadata.error_value().FormatDescription().c_str());
    return persisted_domain_metadata.error_value().status();
  }

  fpbus::Node node(
      {.name = "power-controller",
       .vid = bind_fuchsia_google_platform::BIND_PLATFORM_DEV_VID_GOOGLE,
       .pid = bind_fuchsia_google_platform::BIND_PLATFORM_DEV_PID_ASTRO,
       .did = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_POWER,
       .metadata = std::vector<fpbus::Metadata>{
           {{
               .id = fuchsia_hardware_amlogic_metadata::PowerMetadata::kSerializableName,
               .data = persisted_amlogic_metadata.value(),
           }},
           {{
               .id = fuchsia_hardware_power::DomainMetadata::kSerializableName,
               .data = persisted_domain_metadata.value(),
           }},
       }});

  const std::vector<fuchsia_driver_framework::BindRule2> kPwmRules = {
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.pwm.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia::ID, static_cast<uint32_t>(S905D2_PWM_AO_D))};
  const std::vector<fuchsia_driver_framework::NodeProperty2> kPwmProps = {
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.pwm.Service"),
      fdf::MakeProperty2(bind_fuchsia_amlogic_platform::PWM_ID,
                         bind_fuchsia_amlogic_platform::PWM_ID_AO_D)};
  const std::vector<fuchsia_driver_framework::ParentSpec2> kParents = {
      fuchsia_driver_framework::ParentSpec2{{kPwmRules, kPwmProps}}};

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('POWR');
  fdf::WireUnownedResult result = pbus.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, node),
      fidl::ToWire(fidl_arena, fuchsia_driver_framework::CompositeNodeSpec{
                                   {.name = "power-controller", .parents2 = kParents}}));

  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send AddCompositeNodeSpec request: %s", result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "Failed to add composite: %s", zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}

zx_status_t Astro::PowerInit() {
  zx_status_t status = AddPowerImpl(pbus_);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to add power-impl composite device: %s", zx_status_get_string(status));
    return status;
  }

  return ZX_OK;
}

}  // namespace astro
