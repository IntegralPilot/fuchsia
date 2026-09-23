// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fidl/fuchsia.hardware.ti.metadata/cpp/fidl.h>
#include <lib/ddk/binding.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/clock/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <bind/fuchsia/ti/platform/cpp/bind.h>
#include <soc/aml-common/aml-audio.h>
#include <soc/aml-meson/g12a-clk.h>
#include <soc/aml-s905d2/s905d2-gpio.h>
#include <soc/aml-s905d2/s905d2-hw.h>

#include "astro-gpios.h"
#include "astro.h"

// Enables BT PCM audio.
#define ENABLE_BT

#ifdef TAS2770_CONFIG_PATH
#include TAS2770_CONFIG_PATH
#endif

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

namespace astro {
namespace fpbus = fuchsia_hardware_platform_bus;

static const std::vector<fpbus::Mmio> audio_mmios{
    {{
        .base = S905D2_EE_AUDIO_BASE,
        .length = S905D2_EE_AUDIO_LENGTH,
    }},
};

static const std::vector<fpbus::Irq> frddr_b_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D2_AUDIO_FRDDR_B),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
};
static const std::vector<fpbus::Irq> toddr_b_irqs{
    {{
        .irq = fpbus::IrqSpec::WithIrq(S905D2_AUDIO_TODDR_B),
        .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
    }},
};

static const std::vector<fpbus::Bti> tdm_btis{
    {{
        .iommu_id = 0,
        .bti_id = BTI_AUDIO_OUT,
    }},
};

const std::vector<fuchsia_driver_framework::BindRule2> kGpioInitRules{
    fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};
const std::vector<fuchsia_driver_framework::NodeProperty2> kGpioInitProps{
    fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};

const std::vector<fuchsia_driver_framework::BindRule2> kClockInitRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_clock::BIND_INIT_STEP_CLOCK),
};
const std::vector<fuchsia_driver_framework::NodeProperty2> kClockInitProps = std::vector{
    fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_clock::BIND_INIT_STEP_CLOCK),
};

const std::vector<fuchsia_driver_framework::BindRule2> kAudioEnableGpioRules{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::ID, static_cast<uint32_t>(GPIO_SOC_AUDIO_EN)),
};
const std::vector<fuchsia_driver_framework::NodeProperty2> kAudioEnableGpioProps{
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-enable"),
};

const std::vector<fuchsia_driver_framework::BindRule2> kCodecRules{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.audio.CodecService"),
    fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_VID,
                            bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_VID_TI),
    fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_DID,
                            bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_DID_TAS2770),
};
const std::vector<fuchsia_driver_framework::NodeProperty2> kCodecProps{
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.audio.CodecService"),
    fdf::MakeProperty2(bind_fuchsia::ID, static_cast<uint32_t>(1)),
};

const std::vector<fuchsia_driver_framework::BindRule2> kI2cRules{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::NAME, "audio_codec"),
};
const std::vector<fuchsia_driver_framework::NodeProperty2> kI2cProps{
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
    fdf::MakeProperty2(bind_fuchsia::NAME, "i2c"),
};

const std::vector<fuchsia_driver_framework::BindRule2> kFaultGpioRules{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::ID, static_cast<uint32_t>(GPIO_AUDIO_SOC_FAULT_L)),
};
const std::vector<fuchsia_driver_framework::NodeProperty2> kFaultGpioProps{
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-fault"),
};

const std::vector<fuchsia_driver_framework::ParentSpec2> kTdmI2sSpec = std::vector{
    fuchsia_driver_framework::ParentSpec2{{kGpioInitRules, kGpioInitProps}},
    fuchsia_driver_framework::ParentSpec2{{kClockInitRules, kClockInitProps}},
    fuchsia_driver_framework::ParentSpec2{{kAudioEnableGpioRules, kAudioEnableGpioProps}},
    fuchsia_driver_framework::ParentSpec2{{kCodecRules, kCodecProps}},
};

const std::vector<fuchsia_driver_framework::ParentSpec2> kParentSpecInit = std::vector{
    fuchsia_driver_framework::ParentSpec2{{kGpioInitRules, kGpioInitProps}},
    fuchsia_driver_framework::ParentSpec2{{kClockInitRules, kClockInitProps}},
};

zx_status_t Astro::AudioInit() {
  using fuchsia_hardware_clockimpl::wire::InitCall;

  fdf::Arena arena('AUDI');
  uint8_t tdm_instance_id = 1;

  clock_init_steps_.push_back(ClockDisable(g12a_clk::CLK_HIFI_PLL));
  clock_init_steps_.push_back(ClockSetRate(g12a_clk::CLK_HIFI_PLL, 768'000'000));
  clock_init_steps_.push_back(ClockEnable(g12a_clk::CLK_HIFI_PLL));

  auto sleep = [](zx::duration delay) {
    return fuchsia_hardware_pinimpl::InitStep::WithDelay(delay.get());
  };

  auto audio_pin = [](uint32_t pin, uint64_t function) {
    return fuchsia_hardware_pinimpl::InitStep::WithCall({{
        .pin = pin,
        .call = fuchsia_hardware_pinimpl::InitCall::WithPinConfig({{
            .function = function,
            .drive_strength_ua = 3'000,
        }}),
    }});
  };

  // TDM pin assignments
  gpio_init_steps_.push_back(audio_pin(S905D2_GPIOA(1), S905D2_GPIOA_1_TDMB_SCLK_FN));
  gpio_init_steps_.push_back(audio_pin(S905D2_GPIOA(2), S905D2_GPIOA_2_TDMB_FS_FN));
  gpio_init_steps_.push_back(audio_pin(S905D2_GPIOA(3), S905D2_GPIOA_3_TDMB_D0_FN));
  gpio_init_steps_.push_back(GpioFunction(S905D2_GPIOA(6), S905D2_GPIOA_6_TDMB_DIN3_FN));

#ifdef ENABLE_BT
  // PCM pin assignments.
  gpio_init_steps_.push_back(GpioFunction(S905D2_GPIOX(8), S905D2_GPIOX_8_TDMA_DIN1_FN));
  gpio_init_steps_.push_back(audio_pin(S905D2_GPIOX(9), S905D2_GPIOX_9_TDMA_D0_FN));
  gpio_init_steps_.push_back(audio_pin(S905D2_GPIOX(10), S905D2_GPIOX_10_TDMA_FS_FN));
  gpio_init_steps_.push_back(audio_pin(S905D2_GPIOX(11), S905D2_GPIOX_11_TDMA_SCLK_FN));
#endif

  // PDM pin assignments
  gpio_init_steps_.push_back(GpioFunction(S905D2_GPIOA(7), S905D2_GPIOA_7_PDM_DCLK_FN));
  gpio_init_steps_.push_back(GpioFunction(S905D2_GPIOA(8), S905D2_GPIOA_8_PDM_DIN0_FN));

  // Hardware Reset of the codec.
  gpio_init_steps_.push_back(GpioOutput(S905D2_GPIOA(5), false));
  gpio_init_steps_.push_back(sleep(zx::msec(1)));
  gpio_init_steps_.push_back(GpioOutput(S905D2_GPIOA(5), true));

  // Output devices.
#ifdef ENABLE_BT
  // Add TDM OUT for BT.
  {
    const std::vector<fpbus::Bti> pcm_out_btis = {
        {{
            .iommu_id = 0,
            .bti_id = BTI_AUDIO_BT_OUT,
        }},
    };
    metadata::AmlConfig metadata = {};
    snprintf(metadata.manufacturer, sizeof(metadata.manufacturer), "Spacely Sprockets");
    snprintf(metadata.product_name, sizeof(metadata.product_name), "astro");
    metadata.is_input = false;
    // Compatible clocks with other TDM drivers.
    metadata.mClockDivFactor = 10;
    metadata.sClockDivFactor = 25;
    metadata.unique_id = AUDIO_STREAM_UNIQUE_ID_BUILTIN_BT;
    metadata.bus = metadata::AmlBus::TDM_A;
    metadata.version = metadata::AmlVersion::kS905D2G;
    metadata.dai.type = metadata::DaiType::Custom;
    metadata.dai.custom_sclk_on_raising = true;
    metadata.dai.custom_frame_sync_sclks_offset = 1;
    metadata.dai.custom_frame_sync_size = 1;
    metadata.dai.bits_per_sample = 16;
    metadata.dai.bits_per_slot = 16;
    metadata.ring_buffer.number_of_channels = 1;
    metadata.dai.number_of_channels = 1;
    metadata.lanes_enable_mask[0] = 1;
    std::vector<fpbus::Metadata> tdm_metadata{
        {{
            .id = std::to_string(DEVICE_METADATA_PRIVATE),
            .data = std::vector<uint8_t>(
                reinterpret_cast<const uint8_t*>(&metadata),
                reinterpret_cast<const uint8_t*>(&metadata) + sizeof(metadata)),
        }},
    };

    fpbus::Node tdm_dev;
    tdm_dev.vid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC;
    tdm_dev.pid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_S905D2;
    tdm_dev.mmio() = audio_mmios;
    tdm_dev.bti() = pcm_out_btis;
    tdm_dev.metadata() = tdm_metadata;
    tdm_dev.name() = "astro-pcm-dai-out";
    tdm_dev.did() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_DAI_OUT;
    auto tdm_spec = fuchsia_driver_framework::CompositeNodeSpec{{
        .name = "aml_tdm_dai_out",
        .parents2 = kParentSpecInit,
    }};
    auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(arena, tdm_dev),
                                                            fidl::ToWire(arena, tdm_spec));
    if (!result.ok()) {
      zxlogf(ERROR, "AddCompositeNodeSpec request failed: %s", result.FormatDescription().data());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "AddCompositeNodeSpec failed: %s", zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }
#endif
  // Add TDM OUT to the codec.
  {
    fuchsia_hardware_ti_metadata::TasMetadata metadata;
#ifdef TAS2770_CONFIG_PATH
    const size_t number_of_writes1 = sizeof(tas2770_init_sequence1) / sizeof(cfg_reg);
    metadata.init_sequence1().reserve(number_of_writes1);
    for (const cfg_reg& reg : tas2770_init_sequence1) {
      metadata.init_sequence1().emplace_back(
          fuchsia_hardware_ti_metadata::Register({.address = reg.offset, .value = reg.value}));
    }
    const size_t number_of_writes2 = sizeof(tas2770_init_sequence2) / sizeof(cfg_reg);
    metadata.init_sequence2().reserve(number_of_writes2);
    for (const cfg_reg& reg : tas2770_init_sequence2) {
      metadata.init_sequence2().emplace_back(
          fuchsia_hardware_ti_metadata::Register({.address = reg.offset, .value = reg.value}));
    }
#endif

    fit::result persisted_metadata = fidl::Persist(metadata);
    if (!persisted_metadata.is_ok()) {
      zxlogf(ERROR, "Failed to persist metadata: %s",
             persisted_metadata.error_value().FormatDescription().c_str());
      return persisted_metadata.error_value().status();
    }

    fpbus::Node dev;
    dev.name() = "audio-codec-48";
    dev.vid() = bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_VID_TI;
    dev.did() = bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_DID_TAS2770;
    dev.metadata() = std::vector<fpbus::Metadata>{
        {{
            .id = std::to_string(DEVICE_METADATA_PRIVATE),
            .data = std::move(persisted_metadata.value()),
        }},
    };
    auto parents = std::vector{
        fuchsia_driver_framework::ParentSpec2{{kI2cRules, kI2cProps}},
        fuchsia_driver_framework::ParentSpec2{{kFaultGpioRules, kFaultGpioProps}},
        fuchsia_driver_framework::ParentSpec2{{kGpioInitRules, kGpioInitProps}},
    };
    auto composite_node_spec = fuchsia_driver_framework::CompositeNodeSpec{
        {.name = "audio-codec-48", .parents2 = parents}};

    fdf::WireUnownedResult result = pbus_.buffer(arena)->AddCompositeNodeSpec(
        fidl::ToWire(arena, dev), fidl::ToWire(arena, composite_node_spec));
    if (!result.ok()) {
      zxlogf(ERROR, "Failed to send AddCompositeNodeSpec request: %s", result.status_string());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "Failed to add composite node spec: %s",
             zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }
  {
    metadata::AmlConfig metadata = {};
    snprintf(metadata.manufacturer, sizeof(metadata.manufacturer), "Spacely Sprockets");
    snprintf(metadata.product_name, sizeof(metadata.product_name), "astro");
    metadata.is_input = false;
    // Compatible clocks with other TDM drivers.
    metadata.mClockDivFactor = 10;
    metadata.sClockDivFactor = 25;
    metadata.unique_id = AUDIO_STREAM_UNIQUE_ID_BUILTIN_SPEAKERS;
    metadata.bus = metadata::AmlBus::TDM_B;
    metadata.version = metadata::AmlVersion::kS905D2G;
    metadata.dai.type = metadata::DaiType::I2s;
    metadata.dai.bits_per_sample = 16;
    metadata.dai.bits_per_slot = 32;

    // We expose a mono ring buffer to clients. However we still use a 2 channels DAI to the codec
    // so we configure the audio engine to only take the one channel and put it in the right slot
    // going out to the codec via I2S.
    metadata.ring_buffer.number_of_channels = 1;
    metadata.lanes_enable_mask[0] = 1;  // One ring buffer channel goes into the right I2S slot.
    metadata.codecs.number_of_codecs = 1;
    metadata.codecs.channels_to_use_bitmask[0] = 2;  // Codec must use the right I2S slot.

    metadata.codecs.types[0] = metadata::CodecType::Tas27xx;
    // Report our external delay based on the chosen frame rate.  Note that these
    // delays were measured on Astro hardware, and should be pretty good, but they
    // will not be perfect.  One reason for this is that we are not taking any
    // steps to align our start time with start of a TDM frame, which will cause
    // up to 1 frame worth of startup error ever time that the output starts.
    // Also note that this is really nothing to worry about.  Hitting our target
    // to within 20.8uSec (for 48k) is pretty good.
    metadata.codecs.number_of_external_delays = 2;
    metadata.codecs.external_delays[0].frequency = 48'000;
    metadata.codecs.external_delays[0].nsecs = ZX_USEC(125);
    metadata.codecs.external_delays[1].frequency = 96'000;
    metadata.codecs.external_delays[1].nsecs = ZX_NSEC(83333);
    metadata.codecs.ring_buffer_channels_to_use_bitmask[0] = 0x1;  // Single speaker uses index 0.
    metadata.codecs.delta_gains[0] = -1.5f;
    std::vector<fpbus::Metadata> tdm_metadata{
        {{
            .id = std::to_string(DEVICE_METADATA_PRIVATE),
            .data = std::vector<uint8_t>(
                reinterpret_cast<const uint8_t*>(&metadata),
                reinterpret_cast<const uint8_t*>(&metadata) + sizeof(metadata)),
        }},
    };

    fpbus::Node tdm_dev;
    tdm_dev.name() = "astro-i2s-audio-out";
    tdm_dev.vid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC;
    tdm_dev.pid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_S905D2;
    tdm_dev.did() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_TDM;
    tdm_dev.instance_id() = tdm_instance_id++;
    tdm_dev.mmio() = audio_mmios;
    tdm_dev.bti() = tdm_btis;
    tdm_dev.irq() = frddr_b_irqs;
    tdm_dev.metadata() = tdm_metadata;
    auto tdm_spec = fuchsia_driver_framework::CompositeNodeSpec{{
        .name = "aml_tdm",
        .parents2 = kTdmI2sSpec,
    }};
    auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(arena, tdm_dev),
                                                            fidl::ToWire(arena, tdm_spec));
    if (!result.ok()) {
      zxlogf(ERROR, "AddCompositeNodeSpec request failed: %s", result.FormatDescription().data());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "AddCompositeNodeSpec failed: %s", zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

  // Input devices.
#ifdef ENABLE_BT
  // Add TDM IN for BT.
  {
    const std::vector<fpbus::Bti> pcm_in_btis{
        {{
            .iommu_id = 0,
            .bti_id = BTI_AUDIO_BT_IN,
        }},
    };
    metadata::AmlConfig metadata = {};
    snprintf(metadata.manufacturer, sizeof(metadata.manufacturer), "Spacely Sprockets");
    snprintf(metadata.product_name, sizeof(metadata.product_name), "astro");
    metadata.is_input = true;
    // Compatible clocks with other TDM drivers.
    metadata.mClockDivFactor = 10;
    metadata.sClockDivFactor = 25;
    metadata.unique_id = AUDIO_STREAM_UNIQUE_ID_BUILTIN_BT;
    metadata.bus = metadata::AmlBus::TDM_A;
    metadata.version = metadata::AmlVersion::kS905D2G;
    metadata.dai.type = metadata::DaiType::Custom;
    metadata.dai.custom_sclk_on_raising = true;
    metadata.dai.custom_frame_sync_sclks_offset = 1;
    metadata.dai.custom_frame_sync_size = 1;
    metadata.dai.bits_per_sample = 16;
    metadata.dai.bits_per_slot = 16;
    metadata.ring_buffer.number_of_channels = 1;
    metadata.dai.number_of_channels = 1;
    metadata.swaps = 0x0200;
    metadata.lanes_enable_mask[1] = 1;
    std::vector<fpbus::Metadata> tdm_metadata{
        {{
            .id = std::to_string(DEVICE_METADATA_PRIVATE),
            .data = std::vector<uint8_t>(
                reinterpret_cast<const uint8_t*>(&metadata),
                reinterpret_cast<const uint8_t*>(&metadata) + sizeof(metadata)),
        }},
    };
    fpbus::Node tdm_dev;
    tdm_dev.vid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC;
    tdm_dev.pid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_S905D2;
    tdm_dev.mmio() = audio_mmios;
    tdm_dev.bti() = pcm_in_btis;
    tdm_dev.metadata() = tdm_metadata;
    tdm_dev.name() = "astro-pcm-dai-in";
    tdm_dev.did() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_DAI_IN;
    auto tdm_spec = fuchsia_driver_framework::CompositeNodeSpec{{
        .name = "aml_tdm_dai_in",
        .parents2 = kParentSpecInit,
    }};
    auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(arena, tdm_dev),
                                                            fidl::ToWire(arena, tdm_spec));
    if (!result.ok()) {
      zxlogf(ERROR, "AddCompositeNodeSpec request failed: %s", result.FormatDescription().data());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "AddCompositeNodeSpec failed: %s", zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

#endif

  // Input device.
  {
    metadata::AmlPdmConfig metadata = {};
    snprintf(metadata.manufacturer, sizeof(metadata.manufacturer), "Spacely Sprockets");
    snprintf(metadata.product_name, sizeof(metadata.product_name), "astro");
    metadata.number_of_channels = 2;
    metadata.version = metadata::AmlVersion::kS905D2G;
    metadata.sysClockDivFactor = 4;
    metadata.dClockDivFactor = 250;
    std::vector<fpbus::Metadata> pdm_metadata{
        {{
            .id = std::to_string(DEVICE_METADATA_PRIVATE),
            .data = std::vector<uint8_t>(
                reinterpret_cast<const uint8_t*>(&metadata),
                reinterpret_cast<const uint8_t*>(&metadata) + sizeof(metadata)),
        }},
    };

    static const std::vector<fpbus::Mmio> pdm_mmios = {
        {{
            .base = S905D2_EE_PDM_BASE,
            .length = S905D2_EE_PDM_LENGTH,
        }},
        {{
            .base = S905D2_EE_AUDIO_BASE,
            .length = S905D2_EE_AUDIO_LENGTH,
        }},
    };

    static const std::vector<fpbus::Bti> pdm_btis{
        {{
            .iommu_id = 0,
            .bti_id = BTI_AUDIO_IN,
        }},
    };

    fpbus::Node dev_in;
    dev_in.name() = "astro-audio-pdm-in";
    dev_in.vid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_VID_AMLOGIC;
    dev_in.pid() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_PID_S905D2;
    dev_in.did() = bind_fuchsia_amlogic_platform::BIND_PLATFORM_DEV_DID_PDM;
    dev_in.mmio() = pdm_mmios;
    dev_in.bti() = pdm_btis;
    dev_in.irq() = toddr_b_irqs;
    dev_in.metadata() = pdm_metadata;

    auto pdm_spec = fuchsia_driver_framework::CompositeNodeSpec{{
        .name = "aml_pdm",
        .parents2 = kParentSpecInit,
    }};
    auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(arena, dev_in),
                                                            fidl::ToWire(arena, pdm_spec));
    if (!result.ok()) {
      zxlogf(ERROR, "AddCompositeNodeSpec Audio(dev_in) request failed: %s",
             result.FormatDescription().data());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "AddCompositeNodeSpec Audio(dev_in) failed: %s",
             zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }
  return ZX_OK;
}

}  // namespace astro
