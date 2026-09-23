// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// WARNING: THIS FILE IS MACHINE GENERATED. DO NOT EDIT.
// Generated from the fuchsia.hardware.gpio banjo file

#ifndef SRC_DEVICES_GPIO_LIB_FUCHSIA_HARDWARE_GPIO_INCLUDE_CPP_BANJO_H_
#define SRC_DEVICES_GPIO_LIB_FUCHSIA_HARDWARE_GPIO_INCLUDE_CPP_BANJO_H_

#include <fuchsia/hardware/gpio/c/banjo.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/zx/interrupt.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/types.h>

#include <ddktl/device-internal.h>

#include "banjo-internal.h"

// DDK gpio-protocol support
//
// :: Proxies ::
//
// ddk::GpioProtocolClient is a simple wrapper around
// gpio_protocol_t. It does not own the pointers passed to it.
//
// :: Mixins ::
//
// ddk::GpioProtocol is a mixin class that simplifies writing DDK drivers
// that implement the gpio protocol. It doesn't set the base protocol.
//
// :: Examples ::
//
// // A driver that implements a ZX_PROTOCOL_GPIO device.
// class GpioDevice;
// using GpioDeviceType = ddk::Device<GpioDevice, /* ddk mixins */>;
//
// class GpioDevice : public GpioDeviceType,
//                      public ddk::GpioProtocol<GpioDevice> {
//   public:
//     GpioDevice(zx_device_t* parent)
//         : GpioDeviceType(parent) {}
//
//     zx_status_t GpioConfigIn(uint32_t flags);
//
//     zx_status_t GpioConfigOut(uint8_t initial_value);
//
//     zx_status_t GpioSetAltFunction(uint64_t function);
//
//     zx_status_t GpioRead(uint8_t* out_value);
//
//     zx_status_t GpioWrite(uint8_t value);
//
//     zx_status_t GpioGetInterrupt(uint32_t flags, zx::interrupt* out_irq);
//
//     zx_status_t GpioReleaseInterrupt();
//
//     zx_status_t GpioSetPolarity(gpio_polarity_t polarity);
//
//     zx_status_t GpioSetDriveStrength(uint64_t ds_ua, uint64_t* out_actual_ds_ua);
//
//     zx_status_t GpioGetDriveStrength(uint64_t* out_value);
//
//     ...
// };

namespace ddk {

template <typename D, typename Base = internal::base_mixin>
class GpioProtocol : public Base {
 public:
  GpioProtocol() {
    internal::CheckGpioProtocolSubclass<D>();
    gpio_protocol_ops_.config_in = GpioConfigIn;
    gpio_protocol_ops_.config_out = GpioConfigOut;
    gpio_protocol_ops_.set_alt_function = GpioSetAltFunction;
    gpio_protocol_ops_.read = GpioRead;
    gpio_protocol_ops_.write = GpioWrite;
    gpio_protocol_ops_.get_interrupt = GpioGetInterrupt;
    gpio_protocol_ops_.release_interrupt = GpioReleaseInterrupt;
    gpio_protocol_ops_.set_polarity = GpioSetPolarity;
    gpio_protocol_ops_.set_drive_strength = GpioSetDriveStrength;
    gpio_protocol_ops_.get_drive_strength = GpioGetDriveStrength;

    if constexpr (internal::is_base_proto<Base>::value) {
      auto dev = static_cast<D*>(this);
      // Can only inherit from one base_protocol implementation.
      ZX_ASSERT(dev->ddk_proto_id_ == 0);
      dev->ddk_proto_id_ = ZX_PROTOCOL_GPIO;
      dev->ddk_proto_ops_ = &gpio_protocol_ops_;
    }
  }

 protected:
  gpio_protocol_ops_t gpio_protocol_ops_ = {};

 private:
  // Configures a GPIO for input.
  static zx_status_t GpioConfigIn(void* ctx, uint32_t flags) {
    auto ret = static_cast<D*>(ctx)->GpioConfigIn(flags);
    return ret;
  }
  // Configures a GPIO for output.
  static zx_status_t GpioConfigOut(void* ctx, uint8_t initial_value) {
    auto ret = static_cast<D*>(ctx)->GpioConfigOut(initial_value);
    return ret;
  }
  // Configures the GPIO pin for an alternate function (I2C, SPI, etc)
  // the interpretation of "function" is platform dependent.
  static zx_status_t GpioSetAltFunction(void* ctx, uint64_t function) {
    auto ret = static_cast<D*>(ctx)->GpioSetAltFunction(function);
    return ret;
  }
  // Reads the current value of a GPIO (0 or 1).
  static zx_status_t GpioRead(void* ctx, uint8_t* out_value) {
    auto ret = static_cast<D*>(ctx)->GpioRead(out_value);
    return ret;
  }
  // Sets the current value of the GPIO (any non-zero value maps to 1).
  static zx_status_t GpioWrite(void* ctx, uint8_t value) {
    auto ret = static_cast<D*>(ctx)->GpioWrite(value);
    return ret;
  }
  // Gets an interrupt object pertaining to a particular GPIO pin.
  static zx_status_t GpioGetInterrupt(void* ctx, uint32_t flags, zx_handle_t* out_irq) {
    zx::interrupt out_irq2;
    auto ret = static_cast<D*>(ctx)->GpioGetInterrupt(flags, &out_irq2);
    *out_irq = out_irq2.release();
    return ret;
  }
  // Release the interrupt.
  static zx_status_t GpioReleaseInterrupt(void* ctx) {
    auto ret = static_cast<D*>(ctx)->GpioReleaseInterrupt();
    return ret;
  }
  // Set GPIO polarity.
  static zx_status_t GpioSetPolarity(void* ctx, gpio_polarity_t polarity) {
    auto ret = static_cast<D*>(ctx)->GpioSetPolarity(polarity);
    return ret;
  }
  // Set GPIO drive strength.
  static zx_status_t GpioSetDriveStrength(void* ctx, uint64_t ds_ua, uint64_t* out_actual_ds_ua) {
    auto ret = static_cast<D*>(ctx)->GpioSetDriveStrength(ds_ua, out_actual_ds_ua);
    return ret;
  }
  // Get GPIO drive strength.
  static zx_status_t GpioGetDriveStrength(void* ctx, uint64_t* out_value) {
    auto ret = static_cast<D*>(ctx)->GpioGetDriveStrength(out_value);
    return ret;
  }
};

class GpioProtocolClient {
 public:
  GpioProtocolClient() : ops_(nullptr), ctx_(nullptr) {}
  GpioProtocolClient(const gpio_protocol_t* proto) : ops_(proto->ops), ctx_(proto->ctx) {}

  GpioProtocolClient(zx_device_t* parent) {
    gpio_protocol_t proto;
    if (device_get_protocol(parent, ZX_PROTOCOL_GPIO, &proto) == ZX_OK) {
      ops_ = proto.ops;
      ctx_ = proto.ctx;
    } else {
      ops_ = nullptr;
      ctx_ = nullptr;
    }
  }

  GpioProtocolClient(zx_device_t* parent, const char* fragment_name) {
    gpio_protocol_t proto;
    if (device_get_fragment_protocol(parent, fragment_name, ZX_PROTOCOL_GPIO, &proto) == ZX_OK) {
      ops_ = proto.ops;
      ctx_ = proto.ctx;
    } else {
      ops_ = nullptr;
      ctx_ = nullptr;
    }
  }

  // Create a GpioProtocolClient from the given parent device + "fragment".
  //
  // If ZX_OK is returned, the created object will be initialized in |result|.
  static zx_status_t CreateFromDevice(zx_device_t* parent, GpioProtocolClient* result) {
    gpio_protocol_t proto;
    zx_status_t status = device_get_protocol(parent, ZX_PROTOCOL_GPIO, &proto);
    if (status != ZX_OK) {
      return status;
    }
    *result = GpioProtocolClient(&proto);
    return ZX_OK;
  }

  // Create a GpioProtocolClient from the given parent device.
  //
  // If ZX_OK is returned, the created object will be initialized in |result|.
  static zx_status_t CreateFromDevice(zx_device_t* parent, const char* fragment_name,
                                      GpioProtocolClient* result) {
    gpio_protocol_t proto;
    zx_status_t status =
        device_get_fragment_protocol(parent, fragment_name, ZX_PROTOCOL_GPIO, &proto);
    if (status != ZX_OK) {
      return status;
    }
    *result = GpioProtocolClient(&proto);
    return ZX_OK;
  }

  void GetProto(gpio_protocol_t* proto) const {
    proto->ctx = ctx_;
    proto->ops = ops_;
  }
  bool is_valid() const { return ops_ != nullptr; }
  void clear() {
    ctx_ = nullptr;
    ops_ = nullptr;
  }

  // Configures a GPIO for input.
  zx_status_t ConfigIn(uint32_t flags) const { return ops_->config_in(ctx_, flags); }

  // Configures a GPIO for output.
  zx_status_t ConfigOut(uint8_t initial_value) const {
    return ops_->config_out(ctx_, initial_value);
  }

  // Configures the GPIO pin for an alternate function (I2C, SPI, etc)
  // the interpretation of "function" is platform dependent.
  zx_status_t SetAltFunction(uint64_t function) const {
    return ops_->set_alt_function(ctx_, function);
  }

  // Reads the current value of a GPIO (0 or 1).
  zx_status_t Read(uint8_t* out_value) const { return ops_->read(ctx_, out_value); }

  // Sets the current value of the GPIO (any non-zero value maps to 1).
  zx_status_t Write(uint8_t value) const { return ops_->write(ctx_, value); }

  // Gets an interrupt object pertaining to a particular GPIO pin.
  zx_status_t GetInterrupt(uint32_t flags, zx::interrupt* out_irq) const {
    return ops_->get_interrupt(ctx_, flags, out_irq->reset_and_get_address());
  }

  // Release the interrupt.
  zx_status_t ReleaseInterrupt() const { return ops_->release_interrupt(ctx_); }

  // Set GPIO polarity.
  zx_status_t SetPolarity(gpio_polarity_t polarity) const {
    return ops_->set_polarity(ctx_, polarity);
  }

  // Set GPIO drive strength.
  zx_status_t SetDriveStrength(uint64_t ds_ua, uint64_t* out_actual_ds_ua) const {
    return ops_->set_drive_strength(ctx_, ds_ua, out_actual_ds_ua);
  }

  // Get GPIO drive strength.
  zx_status_t GetDriveStrength(uint64_t* out_value) const {
    return ops_->get_drive_strength(ctx_, out_value);
  }

 private:
  const gpio_protocol_ops_t* ops_;
  void* ctx_;
};

}  // namespace ddk

#endif  // SRC_DEVICES_GPIO_LIB_FUCHSIA_HARDWARE_GPIO_INCLUDE_CPP_BANJO_H_
