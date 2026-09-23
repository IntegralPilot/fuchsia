// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/amlogic/platform/cpp/bind.h>
#include <bind/fuchsia/amlogic/platform/meson/cpp/bind.h>
#include <bind/fuchsia/amlogic/platform/t931/cpp/bind.h>
#include <bind/fuchsia/arm/platform/cpp/bind.h>
#include <bind/fuchsia/camera/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <bind/fuchsia/isp/cpp/bind.h>
#include <bind/fuchsia/sony/platform/cpp/bind.h>
#include <soc/aml-common/aml-registers.h>
#include <soc/aml-meson/g12b-clk.h>
#include <soc/aml-t931/t931-gpio.h>
#include <soc/aml-t931/t931-hw.h>

#include "sherlock-gpios.h"
#include "sherlock.h"

namespace sherlock {
namespace fpbus = fuchsia_hardware_platform_bus;

namespace {

constexpr uint32_t kClk24MAltFunc = 7;
constexpr uint32_t kClkGpioDriveStrengthUa = 4000;

static const std::vector<fpbus::Mmio> ge2d_mmios{
    {{
        .base = T931_GE2D_BASE,
        .length = T931_GE2D_LENGTH,
    }},
};

static const std::vector<fpbus::Bti> ge2d_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_GE2D,
    }},
};

// IRQ for GE2D
static const std::vector<fpbus::Irq> ge2d_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_MALI_GE2D_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
};

static const fpbus::Node ge2d_dev = []() {
  // GE2D
  fpbus::Node dev = {};
  dev.name() = "ge2d";
  dev.vid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC;
  dev.pid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_T931;
  dev.did() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_GE2D;
  dev.mmio() = ge2d_mmios;
  dev.bti() = ge2d_btis;
  dev.irq() = ge2d_irqs;
  return dev;
}();

static const std::vector<fpbus::Mmio> gdc_mmios{
    {{
        .base = T931_HIU_BASE,
        .length = T931_HIU_LENGTH,
    }},
    {{
        .base = T931_GDC_BASE,
        .length = T931_GDC_LENGTH,
    }},
};

static const std::vector<fpbus::Bti> gdc_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_GDC,
    }},
};

// IRQ for ISP
static const std::vector<fpbus::Irq> gdc_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_MALI_GDC_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
};

static const fpbus::Node gdc_dev = []() {
  // GDC
  fpbus::Node dev = {};
  dev.name() = "gdc";
  dev.vid() = bind_fuchsia_arm_platform::BIND_PLATFORM_DEV_VID_ARM;
  dev.pid() = bind_fuchsia_arm_platform::BIND_PLATFORM_DEV_PID_GDC;
  dev.did() = bind_fuchsia_arm_platform::BIND_PLATFORM_DEV_DID_MALI_IV010;
  dev.mmio() = gdc_mmios;
  dev.bti() = gdc_btis;
  dev.irq() = gdc_irqs;
  return dev;
}();

static const std::vector<fpbus::Bti> isp_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_ISP,
    }},
};

static const std::vector<fpbus::Mmio> isp_mmios{
    {{
        .base = T931_HIU_BASE,
        .length = T931_HIU_LENGTH,
    }},
    {{
        .base = T931_POWER_DOMAIN_BASE,
        .length = T931_POWER_DOMAIN_LENGTH,
    }},
    {{
        .base = T931_MEMORY_PD_BASE,
        .length = T931_MEMORY_PD_LENGTH,
    }},
    {{
        .base = T931_ISP_BASE,
        .length = T931_ISP_LENGTH,
    }},
};

// IRQ for ISP
static const std::vector<fpbus::Irq> isp_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_MALI_ISP_IRQ),
        .mode = fpbus::ZirconInterruptMode::kLevelHigh,
    }},
};

static const fpbus::Node isp_dev = []() {
  // ISP
  fpbus::Node dev = {};
  dev.name() = "isp";
  dev.vid() = PDEV_VID_ARM;
  dev.pid() = PDEV_PID_ARM_ISP;
  dev.did() = PDEV_DID_ARM_MALI_IV009;
  dev.mmio() = isp_mmios;
  dev.bti() = isp_btis;
  dev.irq() = isp_irqs;
  return dev;
}();

static const std::vector<fpbus::Mmio> mipi_mmios{
    {{
        .base = T931_CSI_PHY0_BASE,
        .length = T931_CSI_PHY0_LENGTH,
    }},
    {{
        .base = T931_APHY_BASE,
        .length = T931_APHY_LENGTH,
    }},
    {{
        .base = T931_CSI_HOST0_BASE,
        .length = T931_CSI_HOST0_LENGTH,
    }},
    {{
        .base = T931_MIPI_ADAPTER_BASE,
        .length = T931_MIPI_ADAPTER_LENGTH,
    }},
    {{
        .base = T931_HIU_BASE,
        .length = T931_HIU_LENGTH,
    }},
};

static const std::vector<fpbus::Bti> mipi_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_MIPI,
    }},
};

static const std::vector<fpbus::Irq> mipi_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(T931_MIPI_ADAPTER_IRQ),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
};

// Binding rules for MIPI Driver
static const fpbus::Node mipi_dev = []() {
  // MIPI CSI PHY ADAPTER
  fpbus::Node dev = {};
  dev.name() = "mipi-csi-ff650000";
  dev.vid() = PDEV_VID_AMLOGIC;
  dev.pid() = PDEV_PID_AMLOGIC_T931;
  dev.did() = PDEV_DID_AMLOGIC_MIPI_CSI;
  dev.mmio() = mipi_mmios;
  dev.bti() = mipi_btis;
  dev.irq() = mipi_irqs;
  return dev;
}();

// Binding rules for Sensor Driver
static const fpbus::Node sensor_dev_sherlock = []() {
  fpbus::Node dev = {};
  dev.name() = "imx227-sensor";
  dev.vid() = bind_fuchsia_sony_platform::BIND_PLATFORM_DEV_VID_SONY;
  dev.pid() = bind_fuchsia_sony_platform::BIND_PLATFORM_DEV_PID_IMX227;
  dev.did() = bind_fuchsia_sony_platform::BIND_PLATFORM_DEV_DID_CAMERA_SENSOR;
  return dev;
}();

}  // namespace

// Refer to camera design document for driver
// design and layout details.
zx_status_t Sherlock::CameraInit() {
  // Set GPIO alternate functions.
  gpio_init_steps_.push_back(fuchsia_hardware_pinimpl::InitStep::WithCall({{
      .pin = T931_GPIOAO(10),
      .call = fuchsia_hardware_pinimpl::InitCall::WithPinConfig({{
          .function = kClk24MAltFunc,
          .drive_strength_ua = kClkGpioDriveStrengthUa,
      }}),
  }}));

  fidl::Arena<> fidl_arena;
  fdf::Arena arena('CAME');
  {
    auto composite_spec = fuchsia_driver_framework::CompositeNodeSpec{{
        .name = "mipi-csi-ff650000",
        .parents2 = {},
    }};

    auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(
        fidl::ToWire(fidl_arena, mipi_dev), fidl::ToWire(fidl_arena, composite_spec));
    if (!result.ok()) {
      zxlogf(ERROR, "%s: AddCompositeNodeSpec Camera(mipi_dev) request failed: %s", __func__,
             result.FormatDescription().data());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "%s: AddCompositeNodeSpec Camera(mipi_dev) failed: %s", __func__,
             zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

  auto imx227_sensor_mipicsi_spec = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules = std::vector{fdf::MakeAcceptBindRule(
          bind_fuchsia::PROTOCOL, bind_fuchsia_camera::BIND_PROTOCOL_MIPICSI)},
      .properties = {fdf::MakeProperty2(bind_fuchsia::PROTOCOL,
                                        bind_fuchsia_camera::BIND_PROTOCOL_MIPICSI)},
  }};

  auto imx227_sensor_i2c_spec = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::NAME, "camera_sensor"),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "i2c"),
          },
  }};

  auto imx227_sensor_gpio_reset_spec = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                                      bind_fuchsia_amlogic_platform_t931::GPIOZ_PIN_ID_PIN_0),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-reset"),
          },
  }};

  auto imx227_sensor_gpio_vana_spec = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                                      bind_fuchsia_amlogic_platform_t931::GPIOA_PIN_ID_PIN_6),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-vana"),
          },
  }};

  auto imx227_sensor_gpio_vdig_spec = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID,
                                      bind_fuchsia_amlogic_platform_t931::GPIOZ_PIN_ID_PIN_12),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-vdig"),
          },
  }};

  auto imx227_sensor_clock_sensor_spec = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
              fdf::MakeAcceptBindRule(
                  bind_fuchsia::ID,
                  bind_fuchsia_amlogic_platform_meson::G12B_CLK_ID_CLK_CAM_INCK_24M),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "clock-sensor"),
          },
  }};

  auto imx227_sensor_gpio_init_spec = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP,
                                      bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
          },
  }};

  auto composite_spec = fuchsia_driver_framework::CompositeNodeSpec{{
      .name = "imx227",
      .parents2 = {{
          imx227_sensor_mipicsi_spec,
          imx227_sensor_i2c_spec,
          imx227_sensor_gpio_reset_spec,
          imx227_sensor_gpio_vana_spec,
          imx227_sensor_gpio_vdig_spec,
          imx227_sensor_clock_sensor_spec,
          imx227_sensor_gpio_init_spec,
      }},
  }};

  fdf::WireUnownedResult spec_result = pbus_.buffer(arena)->AddCompositeNodeSpec(
      fidl::ToWire(fidl_arena, sensor_dev_sherlock), fidl::ToWire(fidl_arena, composite_spec));

  if (!spec_result.ok()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Camera(sensor_dev_sherlock) request failed: %s",
           __func__, spec_result.FormatDescription().data());
    return spec_result.status();
  }
  if (spec_result->is_error()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Camera(sensor_dev_sherlock) failed: %s", __func__,
           zx_status_get_string(spec_result->error_value()));
    return spec_result->error_value();
  }

  auto bind_rules = std::vector{
      fdf::MakeAcceptBindRule(bind_fuchsia::PROTOCOL,
                              bind_fuchsia_camera::BIND_PROTOCOL_CAMERA_SENSOR_2),
  };

  auto properties = std::vector{
      fdf::MakeProperty2(bind_fuchsia::PROTOCOL,
                         bind_fuchsia_camera::BIND_PROTOCOL_CAMERA_SENSOR_2),
  };

  auto parents = std::vector{
      fuchsia_driver_framework::ParentSpec2{{
          .bind_rules = bind_rules,
          .properties = properties,
      }},
  };

  composite_spec =
      fuchsia_driver_framework::CompositeNodeSpec{{.name = "gdc", .parents2 = parents}};

  spec_result = pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, gdc_dev),
                                                          fidl::ToWire(fidl_arena, composite_spec));
  if (!spec_result.ok()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Camera(gdc_dev) request failed: %s", __func__,
           spec_result.FormatDescription().data());
    return spec_result.status();
  }
  if (spec_result->is_error()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Camera(gdc_dev) failed: %s", __func__,
           zx_status_get_string(spec_result->error_value()));
    return spec_result->error_value();
  }

  auto ge2d_sensor_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::PROTOCOL,
                                      bind_fuchsia_camera::BIND_PROTOCOL_CAMERA_SENSOR_2),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::PROTOCOL,
                                 bind_fuchsia_camera::BIND_PROTOCOL_CAMERA_SENSOR_2),
          },
  }};

  auto ge2d_canvas_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE,
                                      "fuchsia.hardware.amlogiccanvas.Service"),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.amlogiccanvas.Service"),
          },
  }};

  composite_spec = fuchsia_driver_framework::CompositeNodeSpec{{
      .name = "ge2d",
      .parents2 = {{ge2d_sensor_node, ge2d_canvas_node}},
  }};

  spec_result = pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, ge2d_dev),
                                                          fidl::ToWire(fidl_arena, composite_spec));
  if (!spec_result.ok()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Camera(ge2d_dev) request failed: %s", __func__,
           spec_result.FormatDescription().data());
    return spec_result.status();
  }
  if (spec_result->is_error()) {
    zxlogf(ERROR, "%s: AddCompositeNodeSpec Camera(ge2d_dev) failed: %s", __func__,
           zx_status_get_string(spec_result->error_value()));
    return spec_result->error_value();
  }

  auto isp_sensor_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::PROTOCOL,
                                      bind_fuchsia_camera::BIND_PROTOCOL_CAMERA_SENSOR_2),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::PROTOCOL,
                                 bind_fuchsia_camera::BIND_PROTOCOL_CAMERA_SENSOR_2),
          },
  }};

  auto isp_reset_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.registers.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::NAME,
                                      bind_fuchsia_amlogic_platform::NAME_REGISTER_ISP_RESET),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.registers.Service"),
              fdf::MakeProperty2(bind_fuchsia::NAME, "register-reset"),
          },
  }};

  composite_spec = fuchsia_driver_framework::CompositeNodeSpec{
      {.name = "isp", .parents2 = {{isp_sensor_node, isp_reset_node}}}};

  // Add a composite device for ARM ISP
  spec_result = pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, isp_dev),
                                                          fidl::ToWire(fidl_arena, composite_spec));
  if (!spec_result.ok()) {
    zxlogf(ERROR, "AddCompositeNodeSpec Camera(isp_dev) request failed: %s",
           spec_result.FormatDescription().data());
    return spec_result.status();
  }
  if (spec_result->is_error()) {
    zxlogf(ERROR, "AddCompositeNodeSpec Camera(isp_dev) failed: %s",
           zx_status_get_string(spec_result->error_value()));
    return spec_result->error_value();
  }

  const ddk::BindRule kIspRules[] = {
      ddk::MakeAcceptBindRule(bind_fuchsia::PROTOCOL, bind_fuchsia_isp::BIND_PROTOCOL_DEVICE),

  };

  const ddk::BindRule kGdcRules[] = {
      ddk::MakeAcceptBindRule(bind_fuchsia::PROTOCOL, bind_fuchsia_camera::BIND_PROTOCOL_GDC),
  };

  const ddk::BindRule kGe2dRules[] = {
      ddk::MakeAcceptBindRule(bind_fuchsia::PROTOCOL, bind_fuchsia_camera::BIND_PROTOCOL_GE2D),

  };

  const device_bind_prop_t kIspProperties[] = {
      ddk::MakeProperty(bind_fuchsia::PROTOCOL, bind_fuchsia_isp::BIND_PROTOCOL_DEVICE),
  };

  const device_bind_prop_t kGdcProperties[] = {
      ddk::MakeProperty(bind_fuchsia::PROTOCOL, bind_fuchsia_camera::BIND_PROTOCOL_GDC),
  };

  const device_bind_prop_t kGe2dProperties[] = {
      ddk::MakeProperty(bind_fuchsia::PROTOCOL, bind_fuchsia_camera::BIND_PROTOCOL_GE2D),
  };

  auto node_group = ddk::CompositeNodeSpec(kIspRules, kIspProperties)
                        .AddParentSpec(kGdcRules, kGdcProperties)
                        .AddParentSpec(kGe2dRules, kGe2dProperties);

  zx_status_t status = DdkAddCompositeNodeSpec("camera_controller", node_group);
  if (status != ZX_OK) {
    zxlogf(ERROR, "%s: Camera Controller DdkAddCompositeNodeSpec failed %d", __func__, status);
    return status;
  }

  return status;
}

}  // namespace sherlock
