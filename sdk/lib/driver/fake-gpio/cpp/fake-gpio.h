// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_FAKE_GPIO_CPP_FAKE_GPIO_H_
#define LIB_DRIVER_FAKE_GPIO_CPP_FAKE_GPIO_H_

#include <fidl/fuchsia.hardware.gpio/cpp/fidl.h>
#include <fidl/fuchsia.hardware.gpio/cpp/wire_test_base.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zx/interrupt.h>

#include <queue>

namespace fdf_fake {

class FakeGpio final : public fidl::testing::WireTestBase<fuchsia_hardware_gpio::Gpio> {
 public:
  using ReadCallback = std::function<void(ReadCompleter::Sync& completer, FakeGpio& gpio)>;
  using SetBufferModeCallback = std::function<void(
      SetBufferModeRequestView request, SetBufferModeCompleter::Sync& completer, FakeGpio& gpio)>;

  explicit FakeGpio(
      async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher());

  // fidl::testing::WireTestBase<fuchsia_hardware_gpu::Gpio> implementation.
  void GetInterrupt(GetInterruptRequestView request,
                    GetInterruptCompleter::Sync& completer) override;
  void ConfigureInterrupt(ConfigureInterruptRequestView request,
                          ConfigureInterruptCompleter::Sync& completer) override;
  void SetBufferMode(SetBufferModeRequestView request,
                     SetBufferModeCompleter::Sync& completer) override;
  void Read(ReadCompleter::Sync& completer) override;
  void ReleaseInterrupt(ReleaseInterruptCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_gpio::Gpio> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;
  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  void SetBufferMode(fuchsia_hardware_gpio::BufferMode buffer_mode);

  // Set the interrupt used for GetInterrupt requests to `interrupt`.
  void SetInterrupt(zx::result<zx::interrupt> interrupt);

  // Add `callback` to the queue of callbacks used to handle `Read` requests.
  void PushReadCallback(ReadCallback callback);

  // Add a callback that will return `response` to the queue of callbacks used
  // to handle `Read` requests.
  void PushReadResponse(zx::result<bool> response);

  // Set the default response for `Read` requests if the callback queue for
  // `Read` requests is empty. Set to std::nullopt for no default response.
  void SetDefaultReadResponse(std::optional<zx::result<bool>> response);

  // Set the handler used for responding to SetBufferMode requests to `callback`. If `callback` is
  // std::nullopt then resort to default behavior.
  void SetSetSetBufferModeCallback(std::optional<SetBufferModeCallback> callback);

  fuchsia_hardware_gpio::InterruptOptions interrupt_options() const { return interrupt_options_; }

  fuchsia_hardware_gpio::InterruptMode interrupt_mode() const { return interrupt_mode_; }

  fuchsia_hardware_gpio::BufferMode buffer_mode() const { return buffer_mode_; }

  // Serve the gpio FIDL protocol on dispatcher `dispatcher_` and return a client end that can
  // communicate with the server.
  fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> Connect();

  // Returns a handler that binds incoming gpio service connections to this server implementation
  // running on dispatcher `dispatcher_`.
  fuchsia_hardware_gpio::Service::InstanceHandler CreateInstanceHandler();

 private:
  // Returns the polarity of the current state. Returns high if there isn't a
  // current state.
  fuchsia_hardware_gpio::InterruptMode GetCurrentInterruptMode();

  // Default response for `Read` requests if `read_callbacks_` is empty.
  std::optional<zx::result<bool>> default_read_response_;

  // Queue of callbacks that provide values to respond to `Read` requests with.
  std::queue<ReadCallback> read_callbacks_;

  // Handles SetBufferMode requests. If std::nullopt then resort to default behavior.
  std::optional<SetBufferModeCallback> set_buffer_mode_callback_;

  // Interrupt used for GetInterrupt requests.
  zx::result<zx::interrupt> interrupt_;
  bool interrupt_used_ = false;

  fuchsia_hardware_gpio::InterruptOptions interrupt_options_;
  fuchsia_hardware_gpio::InterruptMode interrupt_mode_;
  fuchsia_hardware_gpio::BufferMode buffer_mode_;

  fidl::ServerBindingGroup<fuchsia_hardware_gpio::Gpio> bindings_;
  async_dispatcher_t* dispatcher_;
};

}  // namespace fdf_fake

#endif  // LIB_DRIVER_FAKE_GPIO_CPP_FAKE_GPIO_H_
