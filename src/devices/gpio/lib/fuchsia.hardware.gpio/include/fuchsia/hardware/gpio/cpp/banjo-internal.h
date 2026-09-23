// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// WARNING: THIS FILE IS MACHINE GENERATED. DO NOT EDIT.
// Generated from the fuchsia.hardware.gpio banjo file

#ifndef SRC_DEVICES_GPIO_LIB_FUCHSIA_HARDWARE_GPIO_INCLUDE_CPP_BANJO_INTERNAL_H_
#define SRC_DEVICES_GPIO_LIB_FUCHSIA_HARDWARE_GPIO_INCLUDE_CPP_BANJO_INTERNAL_H_

#include <type_traits>

namespace ddk {
namespace internal {

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN_WITH_SIGNATURE(has_gpio_protocol_config_in, GpioConfigIn,
                                                    zx_status_t (C::*)(uint32_t flags));

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN_WITH_SIGNATURE(has_gpio_protocol_config_out, GpioConfigOut,
                                                    zx_status_t (C::*)(uint8_t initial_value));

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN_WITH_SIGNATURE(has_gpio_protocol_set_alt_function,
                                                    GpioSetAltFunction,
                                                    zx_status_t (C::*)(uint64_t function));

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN_WITH_SIGNATURE(has_gpio_protocol_read, GpioRead,
                                                    zx_status_t (C::*)(uint8_t* out_value));

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN_WITH_SIGNATURE(has_gpio_protocol_write, GpioWrite,
                                                    zx_status_t (C::*)(uint8_t value));

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN_WITH_SIGNATURE(has_gpio_protocol_get_interrupt,
                                                    GpioGetInterrupt,
                                                    zx_status_t (C::*)(uint32_t flags,
                                                                       zx::interrupt* out_irq));

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN_WITH_SIGNATURE(has_gpio_protocol_release_interrupt,
                                                    GpioReleaseInterrupt, zx_status_t (C::*)());

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN_WITH_SIGNATURE(has_gpio_protocol_set_polarity, GpioSetPolarity,
                                                    zx_status_t (C::*)(gpio_polarity_t polarity));

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN_WITH_SIGNATURE(has_gpio_protocol_set_drive_strength,
                                                    GpioSetDriveStrength,
                                                    zx_status_t (C::*)(uint64_t ds_ua,
                                                                       uint64_t* out_actual_ds_ua));

DDKTL_INTERNAL_DECLARE_HAS_MEMBER_FN_WITH_SIGNATURE(has_gpio_protocol_get_drive_strength,
                                                    GpioGetDriveStrength,
                                                    zx_status_t (C::*)(uint64_t* out_value));

template <typename D>
constexpr void CheckGpioProtocolSubclass() {
  static_assert(internal::has_gpio_protocol_config_in<D>::value,
                "GpioProtocol subclasses must implement "
                "zx_status_t GpioConfigIn(uint32_t flags);");

  static_assert(internal::has_gpio_protocol_config_out<D>::value,
                "GpioProtocol subclasses must implement "
                "zx_status_t GpioConfigOut(uint8_t initial_value);");

  static_assert(internal::has_gpio_protocol_set_alt_function<D>::value,
                "GpioProtocol subclasses must implement "
                "zx_status_t GpioSetAltFunction(uint64_t function);");

  static_assert(internal::has_gpio_protocol_read<D>::value,
                "GpioProtocol subclasses must implement "
                "zx_status_t GpioRead(uint8_t* out_value);");

  static_assert(internal::has_gpio_protocol_write<D>::value,
                "GpioProtocol subclasses must implement "
                "zx_status_t GpioWrite(uint8_t value);");

  static_assert(internal::has_gpio_protocol_get_interrupt<D>::value,
                "GpioProtocol subclasses must implement "
                "zx_status_t GpioGetInterrupt(uint32_t flags, zx::interrupt* out_irq);");

  static_assert(internal::has_gpio_protocol_release_interrupt<D>::value,
                "GpioProtocol subclasses must implement "
                "zx_status_t GpioReleaseInterrupt();");

  static_assert(internal::has_gpio_protocol_set_polarity<D>::value,
                "GpioProtocol subclasses must implement "
                "zx_status_t GpioSetPolarity(gpio_polarity_t polarity);");

  static_assert(internal::has_gpio_protocol_set_drive_strength<D>::value,
                "GpioProtocol subclasses must implement "
                "zx_status_t GpioSetDriveStrength(uint64_t ds_ua, uint64_t* out_actual_ds_ua);");

  static_assert(internal::has_gpio_protocol_get_drive_strength<D>::value,
                "GpioProtocol subclasses must implement "
                "zx_status_t GpioGetDriveStrength(uint64_t* out_value);");
}

}  // namespace internal
}  // namespace ddk

#endif  // SRC_DEVICES_GPIO_LIB_FUCHSIA_HARDWARE_GPIO_INCLUDE_CPP_BANJO_INTERNAL_H_
