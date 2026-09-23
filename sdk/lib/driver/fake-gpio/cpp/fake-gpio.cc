// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/fake-gpio/cpp/fake-gpio.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <zircon/errors.h>

#include <atomic>

namespace fdf_fake {

FakeGpio::FakeGpio(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {
  zx::interrupt interrupt;
  ZX_ASSERT(zx::interrupt::create(zx::resource(ZX_HANDLE_INVALID), 0, ZX_INTERRUPT_VIRTUAL,
                                  &interrupt) == ZX_OK);
  interrupt_ = zx::ok(std::move(interrupt));
}

void FakeGpio::GetInterrupt(GetInterruptRequestView request,
                            GetInterruptCompleter::Sync& completer) {
  if (interrupt_.is_error()) {
    completer.ReplyError(interrupt_.error_value());
    return;
  }

  if (interrupt_used_) {
    completer.ReplyError(ZX_ERR_ALREADY_BOUND);
    return;
  }
  interrupt_used_ = true;

  interrupt_options_ = request->options;

  zx::interrupt interrupt;
  ZX_ASSERT(interrupt_.value().duplicate(ZX_RIGHT_SAME_RIGHTS, &interrupt) == ZX_OK);
  completer.ReplySuccess(std::move(interrupt));
}

void FakeGpio::ConfigureInterrupt(ConfigureInterruptRequestView request,
                                  ConfigureInterruptCompleter::Sync& completer) {
  if (request->config.has_mode()) {
    interrupt_mode_ = request->config.mode();
  }
  completer.ReplySuccess();
}

void FakeGpio::SetBufferMode(SetBufferModeRequestView request,
                             SetBufferModeCompleter::Sync& completer) {
  if (set_buffer_mode_callback_.has_value()) {
    set_buffer_mode_callback_.value()(request, completer, *this);
  } else {
    buffer_mode_ = request->mode;
    completer.ReplySuccess();
  }
}

void FakeGpio::Read(ReadCompleter::Sync& completer) {
  if (buffer_mode_ != fuchsia_hardware_gpio::BufferMode::kInput) {
    fdf::error("Buffer node is not set to input");
    completer.ReplyError(ZX_ERR_BAD_STATE);
    return;
  }

  if (!read_callbacks_.empty()) {
    read_callbacks_.front()(completer, *this);
    read_callbacks_.pop();
    return;
  }

  if (!default_read_response_.has_value()) {
    fdf::error("No read callbacks and default response not set.");
    completer.ReplyError(ZX_ERR_BAD_STATE);
    return;
  }

  const auto& response = default_read_response_.value();
  if (response.is_error()) {
    completer.ReplyError(response.status_value());
  } else {
    completer.ReplySuccess(response.value());
  }
}

void FakeGpio::ReleaseInterrupt(ReleaseInterruptCompleter::Sync& completer) {
  interrupt_used_ = false;
  completer.ReplySuccess();
}

void FakeGpio::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_gpio::Gpio> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  ZX_ASSERT_MSG(false, "Unknown method ordinal 0x%016lx", metadata.method_ordinal);
}

void FakeGpio::SetBufferMode(fuchsia_hardware_gpio::BufferMode buffer_mode) {
  buffer_mode_ = buffer_mode;
}

void FakeGpio::SetInterrupt(zx::result<zx::interrupt> interrupt) {
  interrupt_ = std::move(interrupt);
}

void FakeGpio::PushReadCallback(ReadCallback callback) {
  read_callbacks_.push(std::move(callback));
}

void FakeGpio::PushReadResponse(zx::result<bool> response) {
  read_callbacks_.push([response](ReadCompleter::Sync& completer, FakeGpio& gpio) {
    if (response.is_error()) {
      completer.ReplyError(response.status_value());
    } else {
      completer.ReplySuccess(response.value());
    }
  });
}

void FakeGpio::SetDefaultReadResponse(std::optional<zx::result<bool>> response) {
  default_read_response_ = response;
}

void FakeGpio::SetSetSetBufferModeCallback(std::optional<SetBufferModeCallback> callback) {
  set_buffer_mode_callback_ = std::move(callback);
}

fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> FakeGpio::Connect() {
  auto endpoints = fidl::Endpoints<fuchsia_hardware_gpio::Gpio>::Create();
  bindings_.AddBinding(dispatcher_, std::move(endpoints.server), this, fidl::kIgnoreBindingClosure);
  return std::move(endpoints.client);
}

fuchsia_hardware_gpio::Service::InstanceHandler FakeGpio::CreateInstanceHandler() {
  return fuchsia_hardware_gpio::Service::InstanceHandler(
      {.device = bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure)});
}

}  // namespace fdf_fake
