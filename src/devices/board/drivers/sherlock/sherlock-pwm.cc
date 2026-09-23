// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fidl/fuchsia.hardware.pwm/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>

#include <bind/fuchsia/amlogic/platform/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <sdk/lib/driver/component/cpp/composite_node_spec.h>
#include <sdk/lib/driver/component/cpp/node_add_args.h>
#include <soc/aml-t931/t931-pwm.h>

#include "sherlock-gpios.h"
#include "sherlock.h"

namespace sherlock {
namespace fpbus = fuchsia_hardware_platform_bus;

static const std::vector<fpbus::Mmio> pwm_mmios{
    {{
        .base = T931_PWM_AB_BASE,
        .length = T931_PWM_LENGTH,
    }},
    {{
        .base = T931_PWM_CD_BASE,
        .length = T931_PWM_LENGTH,
    }},
    {{
        .base = T931_PWM_EF_BASE,
        .length = T931_PWM_LENGTH,
    }},
    {{
        .base = T931_AO_PWM_AB_BASE,
        .length = T931_AO_PWM_LENGTH,
    }},
    {{
        .base = T931_AO_PWM_CD_BASE,
        .length = T931_AO_PWM_LENGTH,
    }},
};

static fpbus::Node pwm_dev = []() {
  fpbus::Node dev = {};
  dev.name() = "pwm-ffd1b000";
  dev.vid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC;
  dev.pid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_T931;
  dev.did() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_PWM;
  dev.mmio() = pwm_mmios;
  return dev;
}();

const ddk::BindRule kPwmRules[] = {
    ddk::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.pwm.Service"),
    ddk::MakeAcceptBindRule(bind_fuchsia::ID, static_cast<uint32_t>(T931_PWM_E)),
};

const device_bind_prop_t kPwmProperties[] = {
    ddk::MakeProperty(bind_fuchsia::SERVICE, "fuchsia.hardware.pwm.Service"),
};

const ddk::BindRule kGpioInitRules[] = {
    ddk::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};

const device_bind_prop_t kGpioInitProperties[] = {
    ddk::MakeProperty(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};

const ddk::BindRule kGpioBtRules[] = {
    ddk::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    ddk::MakeAcceptBindRule(bind_fuchsia::ID, static_cast<uint32_t>(GPIO_SOC_BT_REG_ON)),
};

const device_bind_prop_t kGpioBtProperties[] = {
    ddk::MakeProperty(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    ddk::MakeProperty(bind_fuchsia::NAME, "gpio-bt"),
};

zx_status_t Sherlock::PwmInit() {
  gpio_init_steps_.push_back(GpioFunction(GPIO_SOC_WIFI_LPO_32k768, T931_WIFI_LPO_CLK_FN));

  // T931_PWM_AO_B controls VDDEE_800 which is configured by the bootloader.
  // Marked as protect so we don't try to initialize it.
  fuchsia_hardware_pwm::PwmChannelsMetadata metadata = {{{{
      {{.id = T931_PWM_A}},
      {{.id = T931_PWM_B}},
      {{.id = T931_PWM_C}},
      {{.id = T931_PWM_D}},
      {{.id = T931_PWM_E}},
      {{.id = T931_PWM_F}},
      {{.id = T931_PWM_AO_A}},
      {{.id = T931_PWM_AO_B, .skip_init = true}},
      {{.id = T931_PWM_AO_C}},
      {{.id = T931_PWM_AO_D}},
  }}}};

  fit::result persisted_metadata = fidl::Persist(metadata);
  if (persisted_metadata.is_error()) {
    zxlogf(ERROR, "Failed to persist pwm channels metadata: %s",
           persisted_metadata.error_value().FormatDescription().c_str());
    return persisted_metadata.error_value().status();
  }

  const std::vector<fpbus::Metadata> pwm_metadata{
      {{
          .id = fuchsia_hardware_pwm::PwmChannelsMetadata::kSerializableName,
          .data = std::move(persisted_metadata.value()),
      }},
  };
  pwm_dev.metadata() = pwm_metadata;

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('PWM_');
  auto composite_spec = fuchsia_driver_framework::wire::CompositeNodeSpec::Builder(arena)
                            .name("pwm-ffd1b000")
                            .Build();

  auto result =
      pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, pwm_dev), composite_spec);
  if (!result.ok()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Pwm(pwm_dev) request failed: %s", __func__,
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Pwm(pwm_dev) failed: %s", __func__,
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  zx_status_t status =
      DdkAddCompositeNodeSpec("pwm-init", ddk::CompositeNodeSpec(kPwmRules, kPwmProperties)
                                              .AddParentSpec(kGpioInitRules, kGpioInitProperties)
                                              .AddParentSpec(kGpioBtRules, kGpioBtProperties));
  if (status != ZX_OK) {
    zxlogf(ERROR, "DdkAddCompositeNodeSpec failed: %s", zx_status_get_string(status));
    return status;
  }

  return ZX_OK;
}

}  // namespace sherlock
