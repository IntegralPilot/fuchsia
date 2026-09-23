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
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/clock/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/gpio/cpp/bind.h>
#include <bind/fuchsia/ti/platform/cpp/bind.h>
#include <soc/aml-common/aml-audio.h>
#include <soc/aml-meson/sm1-clk.h>
#include <soc/aml-s905d3/s905d3-gpio.h>
#include <soc/aml-s905d3/s905d3-hw.h>

#include "nelson-gpios.h"
#include "nelson.h"

#ifdef TAS5805M_CONFIG_PATH
#include TAS5805M_CONFIG_PATH
#endif

namespace fdf {
using namespace fuchsia_driver_framework;
}  // namespace fdf

// Enables BT PCM audio.
#define ENABLE_BT

namespace nelson {
namespace fpbus = fuchsia_hardware_platform_bus;

// Audio out controller composite node specifications.
const std::vector<fuchsia_driver_framework::BindRule2> kGpioInitRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};
const std::vector<fuchsia_driver_framework::NodeProperty2> kGpioInitProps = std::vector{
    fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_gpio::BIND_INIT_STEP_GPIO),
};

const std::vector<fuchsia_driver_framework::BindRule2> kClockInitRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP, bind_fuchsia_clock::BIND_INIT_STEP_CLOCK),
};
const std::vector<fuchsia_driver_framework::NodeProperty2> kClockInitProps = std::vector{
    fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_clock::BIND_INIT_STEP_CLOCK),
};

const std::vector<fuchsia_driver_framework::BindRule2> kAudioEnableGpioRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::ID, static_cast<uint32_t>(GPIO_SOC_AUDIO_EN)),
};
const std::vector<fuchsia_driver_framework::NodeProperty2> kAudioEnableGpioProps = std::vector{
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-enable"),
};

const std::vector<fuchsia_driver_framework::BindRule2> kOutCodecRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.audio.CodecService"),
    fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_VID,
                            bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_VID_TI),
    fdf::MakeAcceptBindRule(bind_fuchsia::PLATFORM_DEV_DID,
                            bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_DID_TAS58XX),
};
const std::vector<fuchsia_driver_framework::NodeProperty2> kOutCodecProps = std::vector{
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.audio.CodecService"),
    fdf::MakeProperty2(bind_fuchsia::ID, static_cast<uint32_t>(1)),
};

const std::vector<fuchsia_driver_framework::ParentSpec2> kOutControllerParents = std::vector{
    fuchsia_driver_framework::ParentSpec2{{kGpioInitRules, kGpioInitProps}},
    fuchsia_driver_framework::ParentSpec2{{kClockInitRules, kClockInitProps}},
    fuchsia_driver_framework::ParentSpec2{{kAudioEnableGpioRules, kAudioEnableGpioProps}},
    fuchsia_driver_framework::ParentSpec2{{kOutCodecRules, kOutCodecProps}},
};

const std::vector<fuchsia_driver_framework::ParentSpec2> kParentSpecInit = std::vector{
    fuchsia_driver_framework::ParentSpec2{{kGpioInitRules, kGpioInitProps}},
    fuchsia_driver_framework::ParentSpec2{{kClockInitRules, kClockInitProps}},
};

// Codec composite node specifications.
const std::vector<fuchsia_driver_framework::BindRule2> kOutI2cRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::NAME, "codec_p2"),
};
const std::vector<fuchsia_driver_framework::NodeProperty2> kOutI2cProps = std::vector{
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.i2c.Service"),
    fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                       bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_VID_TI),
    fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                       bind_fuchsia_ti_platform::BIND_PLATFORM_DEV_DID_TAS58XX),
};

const std::vector<fuchsia_driver_framework::BindRule2> kFaultGpioRules = std::vector{
    fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeAcceptBindRule(bind_fuchsia::ID, static_cast<uint32_t>(GPIO_AUDIO_SOC_FAULT_L)),
};
const std::vector<fuchsia_driver_framework::NodeProperty2> kFaultGpioProps = std::vector{
    fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.gpio.Service"),
    fdf::MakeProperty2(bind_fuchsia::NAME, "gpio-fault"),
};

zx_status_t Nelson::AudioInit() {
  using fuchsia_hardware_clockimpl::wire::InitCall;

  clock_init_steps_.push_back(ClockDisable(sm1_clk::CLK_HIFI_PLL));
  clock_init_steps_.push_back(ClockSetRate(sm1_clk::CLK_HIFI_PLL, 768'000'000));
  clock_init_steps_.push_back(ClockEnable(sm1_clk::CLK_HIFI_PLL));

  static const std::vector<fpbus::Mmio> audio_mmios{
      {{
          .base = S905D3_EE_AUDIO_BASE,
          .length = S905D3_EE_AUDIO_LENGTH,
      }},
  };

  static const std::vector<fpbus::Bti> btis_out{
      {{
          .iommu_id = 0,
          .bti_id = BTI_AUDIO_OUT,
      }},
  };

  static const std::vector<fpbus::Irq> frddr_b_irqs{
      {{
          .irq = fpbus::IrqSpec::WithIrq(S905D3_AUDIO_FRDDR_B),
          .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
      }},
  };
  static const std::vector<fpbus::Irq> toddr_b_irqs{
      {{
          .irq = fpbus::IrqSpec::WithIrq(S905D3_AUDIO_TODDR_B),
          .mode = fpbus::ZirconInterruptMode::kEdgeHigh,
      }},
  };

  static const std::vector<fpbus::Mmio> pdm_mmios{
      {{
          .base = S905D3_EE_PDM_BASE,
          .length = S905D3_EE_PDM_LENGTH,
      }},
      {{
          .base = S905D3_EE_AUDIO_BASE,
          .length = S905D3_EE_AUDIO_LENGTH,
      }},
  };

  static const std::vector<fpbus::Bti> btis_in{
      {{
          .iommu_id = 0,
          .bti_id = BTI_AUDIO_IN,
      }},
  };

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

  // TDM pin assignments.
  gpio_init_steps_.push_back(audio_pin(GPIO_SOC_I2S_SCLK, S905D3_GPIOA_1_TDMB_SCLK_FN));
  gpio_init_steps_.push_back(audio_pin(GPIO_SOC_I2S_FS, S905D3_GPIOA_2_TDMB_FS_FN));
  gpio_init_steps_.push_back(audio_pin(GPIO_SOC_I2S_DO0, S905D3_GPIOA_3_TDMB_D0_FN));

#ifdef ENABLE_BT
  // PCM pin assignments.
  gpio_init_steps_.push_back(GpioFunction(GPIO_SOC_BT_PCM_IN, S905D3_GPIOX_8_TDMA_DIN1_FN));
  gpio_init_steps_.push_back(audio_pin(GPIO_SOC_BT_PCM_OUT, S905D3_GPIOX_9_TDMA_D0_FN));
  gpio_init_steps_.push_back(audio_pin(GPIO_SOC_BT_PCM_SYNC, S905D3_GPIOX_10_TDMA_FS_FN));
  gpio_init_steps_.push_back(audio_pin(GPIO_SOC_BT_PCM_CLK, S905D3_GPIOX_11_TDMA_SCLK_FN));
#endif

  // PDM pin assignments
  gpio_init_steps_.push_back(GpioFunction(GPIO_SOC_MIC_DCLK, S905D3_GPIOA_7_PDM_DCLK_FN));
  // First 2 MICs.
  gpio_init_steps_.push_back(GpioFunction(GPIO_SOC_MICLR_DIN0, S905D3_GPIOA_8_PDM_DIN0_FN));
  // Third MIC.
  gpio_init_steps_.push_back(GpioFunction(GPIO_SOC_MICLR_DIN1, S905D3_GPIOA_9_PDM_DIN1_FN));

  // Board info.
  fidl::Arena<> fidl_arena;
  fdf::Arena arena('AUDI');
  auto result = pbus_.buffer(arena)->GetBoardInfo();
  if (!result.ok()) {
    zxlogf(ERROR, "NodeAdd Audio(dev_in) request failed: %s", result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "NodeAdd Audio(dev_in) failed: %s", zx_status_get_string(result->error_value()));
    return result->error_value();
  }
  auto board_info = fidl::ToNatural(result->value()->info);

  // Output devices.
  metadata::AmlConfig aml_metadata = {};
  snprintf(aml_metadata.manufacturer, sizeof(aml_metadata.manufacturer), "Spacely Sprockets");
  snprintf(aml_metadata.product_name, sizeof(aml_metadata.product_name), "nelson");
  aml_metadata.is_input = false;
  aml_metadata.mClockDivFactor = 10;
  aml_metadata.sClockDivFactor = 25;
  aml_metadata.unique_id = AUDIO_STREAM_UNIQUE_ID_BUILTIN_SPEAKERS;
  aml_metadata.bus = metadata::AmlBus::TDM_B;
  aml_metadata.version = metadata::AmlVersion::kS905D3G;
  aml_metadata.dai.type = metadata::DaiType::I2s;
  aml_metadata.dai.bits_per_sample = 16;
  aml_metadata.dai.bits_per_slot = 32;

  // We expose a mono ring buffer to clients. However we still use a 2 channels DAI to the codec
  // so we configure the audio engine to only take the one channel and put it in the left slot
  // going out to the codec via I2S.
  aml_metadata.ring_buffer.number_of_channels = 1;
  aml_metadata.swaps = 0x10;              // One ring buffer channel goes into the left I2S slot.
  aml_metadata.lanes_enable_mask[0] = 2;  // One ring buffer channel goes into the left I2S slot.
  aml_metadata.codecs.number_of_codecs = 1;
  aml_metadata.codecs.types[0] = metadata::CodecType::Tas58xx;
  aml_metadata.codecs.channels_to_use_bitmask[0] = 1;  // Codec must use the left I2S slot.
  aml_metadata.codecs.ring_buffer_channels_to_use_bitmask[0] = 0x1;  // Single speaker uses index 0.

  std::vector<fpbus::Metadata> tdm_metadata{
      {{
          .id = std::to_string(DEVICE_METADATA_PRIVATE),
          .data = std::vector<uint8_t>(
              reinterpret_cast<const uint8_t*>(&aml_metadata),
              reinterpret_cast<const uint8_t*>(&aml_metadata) + sizeof(aml_metadata)),
      }},
  };

  fpbus::Node controller_out;
  controller_out.name() = "nelson-audio-i2s-out";
  controller_out.vid() = PDEV_VID_AMLOGIC;
  controller_out.pid() = PDEV_PID_AMLOGIC_S905D3;
  controller_out.did() = PDEV_DID_AMLOGIC_TDM;
  controller_out.mmio() = audio_mmios;
  controller_out.bti() = btis_out;
  controller_out.irq() = frddr_b_irqs;
  controller_out.metadata() = tdm_metadata;

  // CODEC pin assignments.
  gpio_init_steps_.push_back(GpioFunction(GPIO_INRUSH_EN_SOC, 0));   // BOOST_EN_SOC as GPIO.
  gpio_init_steps_.push_back(GpioOutput(GPIO_INRUSH_EN_SOC, true));  // BOOST_EN_SOC to high.
  // From the TAS5805m codec reference manual:
  // "9.5.3.1 Startup Procedures
  // 1. Configure ADR/FAULT pin with proper settings for I2C device address.
  // 2. Bring up power supplies (it does not matter if PVDD or DVDD comes up first).
  // 3. Once power supplies are stable, bring up PDN to High and wait 5ms at least, then
  // start SCLK, LRCLK.
  // 4. Once I2S clocks are stable, set the device into HiZ state and enable DSP via the I2C
  // control port.
  // 5. Wait 5ms at least. Then initialize the DSP Coefficient, then set the device to Play
  // state.
  // 6. The device is now in normal operation."
  // Step 3 PDN setup and 5ms delay is executed below.
  gpio_init_steps_.push_back(GpioOutput(GPIO_SOC_AUDIO_EN, true));  // Set PDN_N to high.
  gpio_init_steps_.push_back(sleep(zx::msec(5)));
  // I2S clocks are configured by the controller and the rest of the initialization is done
  // in the codec itself.
  fuchsia_hardware_ti_metadata::TasMetadata metadata({.bridged = true});
#ifdef TAS5805M_CONFIG_PATH
  const size_t number_of_writes1 = sizeof(tas5805m_init_sequence1) / sizeof(cfg_reg);
  metadata.init_sequence1().reserve(number_of_writes1);
  for (const cfg_reg& reg : tas5805m_init_sequence1) {
    metadata.init_sequence1().emplace_back(
        fuchsia_hardware_ti_metadata::Register({.address = reg.offset, .value = reg.value}));
  }
  const size_t number_of_writes2 = sizeof(tas5805m_init_sequence2) / sizeof(cfg_reg);
  metadata.init_sequence2().reserve(number_of_writes2);
  for (const cfg_reg& reg : tas5805m_init_sequence2) {
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
  dev.name() = "tas58xx";
  dev.vid() = PDEV_VID_TI;
  dev.did() = PDEV_DID_TI_TAS58xx;
  dev.metadata() = std::vector<fpbus::Metadata>{
      {{
          .id = std::to_string(DEVICE_METADATA_PRIVATE),
          .data = std::move(persisted_metadata.value()),
      }},
  };
  auto parents = std::vector{
      fuchsia_driver_framework::ParentSpec2{{kOutI2cRules, kOutI2cProps}},
      fuchsia_driver_framework::ParentSpec2{{kFaultGpioRules, kFaultGpioProps}},
      fuchsia_driver_framework::ParentSpec2{{kGpioInitRules, kGpioInitProps}},
  };
  {
    auto composite_node_spec =
        fuchsia_driver_framework::CompositeNodeSpec{{.name = "tas58xx", .parents2 = parents}};
    fdf::WireUnownedResult result = pbus_.buffer(arena)->AddCompositeNodeSpec(
        fidl::ToWire(fidl_arena, dev), fidl::ToWire(fidl_arena, composite_node_spec));
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
    auto controller_out_spec = fuchsia_driver_framework::CompositeNodeSpec{{
        .name = "aml_tdm",
        .parents2 = kOutControllerParents,
    }};
    auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(
        fidl::ToWire(fidl_arena, controller_out), fidl::ToWire(fidl_arena, controller_out_spec));
    if (!result.ok()) {
      zxlogf(ERROR, "AddCompositeNodeSpec Audio(controller_out) request failed: %s",
             result.FormatDescription().data());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "AddCompositeNodeSpec Audio(controller_out) failed: %s",
             zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }

#ifdef ENABLE_BT
  // Add TDM OUT for BT.
  {
    static const std::vector<fpbus::Bti> pcm_out_btis{
        {{
            .iommu_id = 0,
            .bti_id = BTI_AUDIO_BT_OUT,
        }},
    };
    metadata::AmlConfig metadata = {};
    snprintf(metadata.manufacturer, sizeof(metadata.manufacturer), "Spacely Sprockets");
    snprintf(metadata.product_name, sizeof(metadata.product_name), "nelson");

    metadata.is_input = false;
    // Compatible clocks with other TDM drivers.
    metadata.mClockDivFactor = 10;
    metadata.sClockDivFactor = 25;
    metadata.unique_id = AUDIO_STREAM_UNIQUE_ID_BUILTIN_BT;
    metadata.bus = metadata::AmlBus::TDM_A;
    metadata.version = metadata::AmlVersion::kS905D3G;
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
    tdm_dev.name() = "nelson-pcm-dai-out";
    tdm_dev.vid() = PDEV_VID_AMLOGIC;
    tdm_dev.pid() = PDEV_PID_AMLOGIC_S905D3;
    tdm_dev.did() = PDEV_DID_AMLOGIC_DAI_OUT;
    tdm_dev.mmio() = audio_mmios;
    tdm_dev.bti() = pcm_out_btis;
    tdm_dev.metadata() = tdm_metadata;

    auto tdm_spec = fuchsia_driver_framework::CompositeNodeSpec{{
        .name = "aml_tdm_dai_out",
        .parents2 = kParentSpecInit,
    }};
    auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, tdm_dev),
                                                            fidl::ToWire(fidl_arena, tdm_spec));
    if (!result.ok()) {
      zxlogf(ERROR, "AddCompositeNodeSpec Audio(tdm_dev) request failed: %s",
             result.FormatDescription().data());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "AddCompositeNodeSpec Audio(tdm_dev) failed: %s",
             zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }
#endif

  // Input devices.
#ifdef ENABLE_BT
  // Add TDM IN for BT.
  {
    static const std::vector<fpbus::Bti> pcm_in_btis{
        {{
            .iommu_id = 0,
            .bti_id = BTI_AUDIO_BT_IN,
        }},
    };
    metadata::AmlConfig metadata = {};
    snprintf(metadata.manufacturer, sizeof(metadata.manufacturer), "Spacely Sprockets");
    snprintf(metadata.product_name, sizeof(metadata.product_name), "nelson");
    metadata.is_input = true;
    // Compatible clocks with other TDM drivers.
    metadata.mClockDivFactor = 10;
    metadata.sClockDivFactor = 25;
    metadata.unique_id = AUDIO_STREAM_UNIQUE_ID_BUILTIN_BT;
    metadata.bus = metadata::AmlBus::TDM_A;
    metadata.version = metadata::AmlVersion::kS905D3G;
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
    tdm_dev.name() = "nelson-pcm-dai-in";
    tdm_dev.vid() = PDEV_VID_AMLOGIC;
    tdm_dev.pid() = PDEV_PID_AMLOGIC_S905D3;
    tdm_dev.did() = PDEV_DID_AMLOGIC_DAI_IN;
    tdm_dev.mmio() = audio_mmios;
    tdm_dev.bti() = pcm_in_btis;
    tdm_dev.metadata() = tdm_metadata;

    auto tdm_spec = fuchsia_driver_framework::CompositeNodeSpec{{
        .name = "aml_tdm_dai_in",
        .parents2 = kParentSpecInit,
    }};
    auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, tdm_dev),
                                                            fidl::ToWire(fidl_arena, tdm_spec));
    if (!result.ok()) {
      zxlogf(ERROR, "AddCompositeNodeSpec Audio(tdm_dev) request failed: %s",
             result.FormatDescription().data());
      return result.status();
    }
    if (result->is_error()) {
      zxlogf(ERROR, "AddCompositeNodeSpec Audio(tdm_dev) failed: %s",
             zx_status_get_string(result->error_value()));
      return result->error_value();
    }
  }
#endif

  // PDM.
  {
    metadata::AmlPdmConfig metadata = {};
    snprintf(metadata.manufacturer, sizeof(metadata.manufacturer), "Spacely Sprockets");
    snprintf(metadata.product_name, sizeof(metadata.product_name), "nelson");
    metadata.number_of_channels = 3;
    metadata.version = metadata::AmlVersion::kS905D3G;
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

    fpbus::Node dev_in;
    dev_in.name() = "nelson-audio-pdm-in";
    dev_in.vid() = PDEV_VID_AMLOGIC;
    dev_in.pid() = PDEV_PID_AMLOGIC_S905D3;
    dev_in.did() = PDEV_DID_AMLOGIC_PDM;
    dev_in.mmio() = pdm_mmios;
    dev_in.bti() = btis_in;
    dev_in.irq() = toddr_b_irqs;
    dev_in.metadata() = pdm_metadata;

    auto pdm_spec = fuchsia_driver_framework::CompositeNodeSpec{{
        .name = "aml_pdm",
        .parents2 = kParentSpecInit,
    }};
    auto result = pbus_.buffer(arena)->AddCompositeNodeSpec(fidl::ToWire(fidl_arena, dev_in),
                                                            fidl::ToWire(fidl_arena, pdm_spec));
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

}  // namespace nelson
