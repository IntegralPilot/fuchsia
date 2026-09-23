// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "fake-gpio.h"

#include <fidl/fuchsia.hardware.gpio/cpp/common_types.h>
#include <lib/async/default.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <zircon/errors.h>

#include <atomic>
#include <variant>

namespace {

// Get the correct dispatcher for the test environment
async_dispatcher_t* GetDefaultDispatcher() {
  async_dispatcher_t* current_fdf_dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
  if (current_fdf_dispatcher) {
    return current_fdf_dispatcher;
  }

  return async_get_default_dispatcher();
}

}  // anonymous namespace

namespace fake_gpio {

bool WriteSubState::operator==(const WriteSubState& other) const { return value == other.value; }

bool ReadSubState::operator==(const ReadSubState& other) const { return true; }

bool AltFunctionSubState::operator==(const AltFunctionSubState& other) const {
  return function == other.function;
}

zx_status_t DefaultSetBufferModeCallback(FakeGpio& gpio) { return ZX_OK; }

FakeGpio::FakeGpio() : set_buffer_mode_callback_(DefaultSetBufferModeCallback) {
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

  if (interrupt_used_.exchange(/*desired=*/true, std::memory_order_relaxed)) {
    completer.ReplyError(ZX_ERR_ALREADY_BOUND);
    return;
  }

  if (state_log_.empty()) {
    state_log_.emplace_back(
        State{.interrupt_options = request->options, .sub_state = ReadSubState{}});
  } else {
    state_log_.back().interrupt_options = request->options;
  }
  zx::interrupt interrupt;
  ZX_ASSERT(interrupt_.value().duplicate(ZX_RIGHT_SAME_RIGHTS, &interrupt) == ZX_OK);
  completer.ReplySuccess(std::move(interrupt));
}

void FakeGpio::ConfigureInterrupt(ConfigureInterruptRequestView request,
                                  ConfigureInterruptCompleter::Sync& completer) {
  if (request->config.has_mode()) {
    auto sub_state = state_log_.empty() ? ReadSubState{} : state_log_.back().sub_state;
    state_log_.emplace_back(State{
        .interrupt_mode = request->config.mode(),
        .sub_state = sub_state,
    });
  }
  completer.ReplySuccess();
}

void FakeGpio::SetBufferMode(SetBufferModeRequestView request,
                             SetBufferModeCompleter::Sync& completer) {
  switch (request->mode) {
    case fuchsia_hardware_gpio::BufferMode::kInput:
      if (state_log_.empty() ||
          !std::holds_alternative<ReadSubState>(state_log_.back().sub_state)) {
        state_log_.emplace_back(
            State{.interrupt_mode = GetCurrentInterruptMode(), .sub_state = ReadSubState{}});
      }
      break;
    case fuchsia_hardware_gpio::BufferMode::kOutputLow:
    case fuchsia_hardware_gpio::BufferMode::kOutputHigh:
      state_log_.emplace_back(State{
          .interrupt_mode = GetCurrentInterruptMode(),
          .sub_state = WriteSubState{.value = request->mode ==
                                              fuchsia_hardware_gpio::BufferMode::kOutputHigh}});
      break;
    default:
      ZX_ASSERT_MSG(false, "Unepxected BufferMode value");
  }

  zx_status_t response = set_buffer_mode_callback_(*this);
  if (response == ZX_OK) {
    completer.ReplySuccess();
  } else {
    completer.ReplyError(response);
  }
}

void FakeGpio::Read(ReadCompleter::Sync& completer) {
  ZX_ASSERT(std::holds_alternative<ReadSubState>(state_log_.back().sub_state));
  zx::result<bool> response;
  if (read_callbacks_.empty()) {
    ZX_ASSERT(default_read_response_.has_value());
    response = default_read_response_.value();
  } else {
    response = read_callbacks_.front()(*this);
    read_callbacks_.pop();
  }
  if (response.is_ok()) {
    completer.ReplySuccess(response.value());
  } else {
    completer.ReplyError(response.error_value());
  }
}

void FakeGpio::ReleaseInterrupt(ReleaseInterruptCompleter::Sync& completer) {
  interrupt_used_.store(false);
  completer.ReplySuccess();
}

void FakeGpio::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_gpio::Gpio> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  ZX_ASSERT_MSG(false, "Unknown method ordinal 0x%016lx", metadata.method_ordinal);
}

uint64_t FakeGpio::GetAltFunction() const {
  const auto& state = std::get<AltFunctionSubState>(state_log_.back().sub_state);
  return state.function;
}

uint8_t FakeGpio::GetWriteValue() const {
  ZX_ASSERT(!state_log_.empty());
  const auto& state = std::get<WriteSubState>(state_log_.back().sub_state);
  return state.value;
}

fuchsia_hardware_gpio::InterruptMode FakeGpio::GetInterruptMode() const {
  return state_log_.back().interrupt_mode;
}

void FakeGpio::SetInterrupt(zx::result<zx::interrupt> interrupt) {
  interrupt_ = std::move(interrupt);
}

void FakeGpio::PushReadCallback(ReadCallback callback) {
  read_callbacks_.push(std::move(callback));
}

void FakeGpio::PushReadResponse(zx::result<bool> response) {
  read_callbacks_.push([response](FakeGpio& gpio) { return response; });
}

void FakeGpio::SetDefaultReadResponse(std::optional<zx::result<bool>> response) {
  default_read_response_ = response;
}

void FakeGpio::SetSetBufferModeCallback(SetBufferModeCallback set_buffer_mode_callback) {
  set_buffer_mode_callback_ = std::move(set_buffer_mode_callback);
}

void FakeGpio::SetCurrentState(State state) { state_log_.push_back(std::move(state)); }

std::vector<State> FakeGpio::GetStateLog() { return state_log_; }

fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> FakeGpio::Connect() {
  auto endpoints = fidl::Endpoints<fuchsia_hardware_gpio::Gpio>::Create();
  bindings_.AddBinding(GetDefaultDispatcher(), std::move(endpoints.server), this,
                       fidl::kIgnoreBindingClosure);
  return std::move(endpoints.client);
}

fuchsia_hardware_gpio::Service::InstanceHandler FakeGpio::CreateInstanceHandler() {
  return fuchsia_hardware_gpio::Service::InstanceHandler(
      {.device =
           bindings_.CreateHandler(this, GetDefaultDispatcher(), fidl::kIgnoreBindingClosure)});
}

fuchsia_hardware_gpio::InterruptMode FakeGpio::GetCurrentInterruptMode() {
  if (state_log_.empty()) {
    return fuchsia_hardware_gpio::InterruptMode::kEdgeHigh;
  }
  return state_log_.back().interrupt_mode;
}

}  // namespace fake_gpio
