// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "gpioutil.h"

#include <dirent.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/fdio/directory.h>
#include <lib/fit/defer.h>
#include <stdio.h>
#include <zircon/types.h>

#include <filesystem>
#include <optional>
#include <print>
#include <string_view>

// Directory path to the GPIO class
#define GPIO_DEV_CLASS_PATH "/dev/class/gpio/"
constexpr char kGpioDevClassDir[] = GPIO_DEV_CLASS_PATH;
constexpr char kGpioDevClassNsDir[] = "/ns" GPIO_DEV_CLASS_PATH;

constexpr std::optional<fuchsia_hardware_pin::Pull> ParsePull(std::string_view arg) {
  if (arg == "down") {
    return fuchsia_hardware_pin::Pull::kDown;
  }
  if (arg == "up") {
    return fuchsia_hardware_pin::Pull::kUp;
  }
  if (arg == "none") {
    return fuchsia_hardware_pin::Pull::kNone;
  }
  return {};
}

constexpr std::optional<fuchsia_hardware_gpio::InterruptMode> ParseInterruptFlags(
    std::string_view arg) {
  if (arg == "edge-low") {
    return fuchsia_hardware_gpio::InterruptMode::kEdgeLow;
  }
  if (arg == "edge-high") {
    return fuchsia_hardware_gpio::InterruptMode::kEdgeHigh;
  }
  if (arg == "edge-both") {
    return fuchsia_hardware_gpio::InterruptMode::kEdgeBoth;
  }
  if (arg == "level-low") {
    return fuchsia_hardware_gpio::InterruptMode::kLevelLow;
  }
  if (arg == "level-high") {
    return fuchsia_hardware_gpio::InterruptMode::kLevelHigh;
  }
  return {};
}

int ParseArgs(int argc, char** argv, GpioFunc* func, fidl::AnyArena& arena,
              fuchsia_hardware_gpio::BufferMode* buffer_mode,
              fuchsia_hardware_gpio::InterruptMode* interrupt_mode,
              fuchsia_hardware_pin::wire::Configuration* config) {
  if (argc < 2) {
    return -1;
  }

  std::string_view func_arg = argv[1];

  /* Following functions allow no args */
  switch (func_arg[0]) {
    case 'l':
      *func = List;
      return 0;
  }

  if (argc < 3) {
    return -1;
  }

  *buffer_mode = {};
  *interrupt_mode = {};
  switch (func_arg[0]) {
    case 'n':
      *func = GetName;
      break;
    case 'r':
      *func = Read;
      break;
    case 'i':
    case 'q':
      if (func_arg[0] == 'q' || func_arg == "interrupt") {
        if (argc < 4) {
          return -1;
        }

        *func = Interrupt;
        std::optional<fuchsia_hardware_gpio::InterruptMode> parsed_interrupt_mode =
            ParseInterruptFlags(argv[3]);
        if (!parsed_interrupt_mode) {
          std::println(stderr, "Invalid interrupt flag \"{}\"\n", argv[3]);
          return -1;
        }

        *interrupt_mode = *parsed_interrupt_mode;
      } else {
        if (argc < 3) {
          return -1;
        }

        *func = SetBufferMode;
        *buffer_mode = fuchsia_hardware_gpio::BufferMode::kInput;
      }
      break;
    case 'o':
      *func = SetBufferMode;

      if (argc < 4) {
        return -1;
      }

      *buffer_mode = std::stoul(argv[3]) ? fuchsia_hardware_gpio::BufferMode::kOutputHigh
                                         : fuchsia_hardware_gpio::BufferMode::kOutputLow;
      break;
    case 'd':
      if (argc >= 4) {
        *func = Configure;
        *config = fuchsia_hardware_pin::wire::Configuration::Builder(arena)
                      .drive_strength_ua(static_cast<uint64_t>(std::stoull(argv[3])))
                      .Build();
      } else if (argc == 3) {
        *func = GetDriveStrength;
      } else {
        return -1;
      }

      break;
    case 'f': {
      *func = Configure;

      if (argc < 4) {
        return -1;
      }

      char* endptr;
      uint64_t func_val = std::strtoull(argv[3], &endptr, 10);
      if (*endptr == '\0') {
        *config =
            fuchsia_hardware_pin::wire::Configuration::Builder(arena).function(func_val).Build();
      } else {
        *config = fuchsia_hardware_pin::wire::Configuration::Builder(arena)
                      .function_name(fidl::StringView::FromExternal(argv[3]))
                      .Build();
      }
      break;
    }
    case 'p': {
      *func = Configure;

      if (argc < 4) {
        return -1;
      }

      std::optional<fuchsia_hardware_pin::Pull> parsed_pull = ParsePull(argv[3]);
      if (!parsed_pull) {
        std::println(stderr, "Invalid pull value \"{}\"\n", argv[3]);
        return -1;
      }

      *config =
          fuchsia_hardware_pin::wire::Configuration::Builder(arena).pull(*parsed_pull).Build();
      break;
    }
    default:
      *func = Invalid;
      return -1;
  }

  return 0;
}

zx::result<> ListGpios() {
  const char* dev_class_dir = kGpioDevClassNsDir;
  if (!std::filesystem::is_directory(dev_class_dir)) {
    dev_class_dir = kGpioDevClassDir;
  }

  for (auto const& dir_entry : std::filesystem::directory_iterator(dev_class_dir)) {
    const char* gpio_path = dir_entry.path().c_str();
    zx::result client_end = component::Connect<fuchsia_hardware_pin::Debug>(gpio_path);
    if (client_end.is_error()) {
      std::println(stderr, "Could not connect to client from {}: {}", gpio_path, client_end);
      return client_end.take_error();
    }

    fidl::WireSyncClient<fuchsia_hardware_pin::Debug> client(std::move(client_end.value()));
    const fidl::WireResult result = client->GetProperties();
    if (!result.ok()) {
      std::println(stderr, "Could not get properties from {}: {}", gpio_path, result.error());
      return zx::error(result.status());
    }

    uint32_t pin = result->pin();
    std::string_view name = result->name().get();
    printf("[gpio-%d] %.*s\n", pin, static_cast<int>(name.length()), name.data());
  }

  return zx::ok();
}

zx::result<fidl::WireSyncClient<fuchsia_hardware_pin::Debug>> FindDebugClientByName(
    std::string_view name) {
  const char* dev_class_dir = kGpioDevClassNsDir;
  if (!std::filesystem::is_directory(dev_class_dir)) {
    dev_class_dir = kGpioDevClassDir;
  }

  for (auto const& dir_entry : std::filesystem::directory_iterator(dev_class_dir)) {
    const char* gpio_path = dir_entry.path().c_str();

    zx::result client_end = component::Connect<fuchsia_hardware_pin::Debug>(gpio_path);
    if (client_end.is_error()) {
      std::println(stderr, "Could not connect to client from {}: {}", gpio_path, client_end);
      return client_end.take_error();
    }

    fidl::WireSyncClient<fuchsia_hardware_pin::Debug> client(std::move(client_end.value()));

    const fidl::WireResult result_name = client->GetProperties();
    if (!result_name.ok()) {
      std::println(stderr, "Could not get properties from {}: {}", gpio_path, result_name.error());
      return zx::error(result_name.status());
    }
    std::string_view gpio_name = result_name->name().get();
    if (name == gpio_name) {
      return zx::ok(std::move(client));
    }
  }

  return zx::error(ZX_ERR_NOT_FOUND);
}

int ClientCall(fidl::WireSyncClient<fuchsia_hardware_pin::Debug> client, GpioFunc func,
               fidl::AnyArena& arena, fuchsia_hardware_gpio::BufferMode buffer_mode,
               fuchsia_hardware_gpio::InterruptMode interrupt_mode,
               fuchsia_hardware_pin::wire::Configuration config) {
  if (func == GetName) {
    auto result = client->GetProperties();
    if (!result.ok()) {
      std::println(stderr, "Call to get properties failed: {}", result.error());
      return -2;
    }
    auto pin = result->pin();
    auto name = result->name().get();
    std::println("GPIO Name: [gpio-{}] {}", pin, name);
    return 0;
  }

  auto [gpio_client_end, gpio_server_end] = fidl::Endpoints<fuchsia_hardware_gpio::Gpio>::Create();
  fidl::WireSyncClient<fuchsia_hardware_gpio::Gpio> gpio_client(std::move(gpio_client_end));

  if (auto result = client->ConnectGpio(std::move(gpio_server_end)); !result.ok()) {
    std::println(stderr, "Call to connect GPIO failed: {}", result.error());
    return -2;
  } else if (result->is_error()) {
    std::println(stderr, "Failed to connect GPIO: {}", zx::make_result(result->error_value()));
    return -2;
  }

  auto [pin_client_end, pin_server_end] = fidl::Endpoints<fuchsia_hardware_pin::Pin>::Create();
  fidl::WireSyncClient<fuchsia_hardware_pin::Pin> pin_client(std::move(pin_client_end));

  if (auto result = client->ConnectPin(std::move(pin_server_end)); !result.ok()) {
    std::println(stderr, "Call to connect pin failed: {}", result.error());
    return -2;
  } else if (result->is_error()) {
    std::println(stderr, "Failed to connect pin: {}", zx::make_result(result->error_value()));
    return -2;
  }

  switch (func) {
    case Read: {
      auto result = gpio_client->Read();
      if (!result.ok()) {
        std::println(stderr, "Call to read GPIO failed: {}", result.error());
        return -2;
      }
      if (result->is_error()) {
        std::println(stderr, "Could not read GPIO: {}", zx::make_result(result->error_value()));
        return -2;
      }
      std::println("GPIO Value: {}", result->value()->value);
      break;
    }
    case SetBufferMode: {
      auto result = gpio_client->SetBufferMode(buffer_mode);
      if (!result.ok()) {
        std::println(stderr, "Call to set GPIO buffer mode failed: {}", result.error());
        return -2;
      }
      if (result->is_error()) {
        std::println(stderr, "Could not set GPIO buffer mode: {}",
                     zx::make_result(result->error_value()));
        return -2;
      }
      break;
    }
    case Interrupt: {
      auto interrupt_config = fuchsia_hardware_gpio::wire::InterruptConfiguration::Builder(arena)
                                  .mode(interrupt_mode)
                                  .Build();
      if (auto result = gpio_client->ConfigureInterrupt(interrupt_config); !result.ok()) {
        std::println(stderr, "Call to get GPIO interrupt failed: {}", result.error());
        return -2;
      } else if (result->is_error()) {
        std::println(stderr, "Could not get GPIO interrupt: {}",
                     zx::make_result(result->error_value()));
        return -2;
      }

      auto result = gpio_client->GetInterrupt({});
      if (!result.ok()) {
        std::println(stderr, "Call to get GPIO interrupt failed: {}", result.error());
        return -2;
      }
      if (result->is_error()) {
        std::println(stderr, "Could not get GPIO interrupt: {}",
                     zx::make_result(result->error_value()));
        return -2;
      }

      zx::time_boot timestamp{};
      if (zx_status_t status = result->value()->interrupt.wait(&timestamp); status != ZX_OK) {
        std::println(stderr, "Interrupt wait failed: {}", zx::make_result(status));
        return -2;
      }

      printf("Received interrupt at time %ld\n", timestamp.get());

      auto release = gpio_client->ReleaseInterrupt();
      if (!release.ok()) {
        std::println(stderr, "Call to release GPIO interrupt failed: {}", release.error());
        return -2;
      }
      if (release->is_error()) {
        std::println(stderr, "Could not release GPIO interrupt: {}",
                     zx::make_result(release->error_value()));
        return -2;
      }
      break;
    }
    case Configure: {
      auto result = pin_client->Configure(config);
      if (!result.ok()) {
        std::println(stderr, "Call to configure pin failed: {}", result.error());
        return -2;
      }
      if (result->is_error()) {
        std::println(stderr, "Could not configure pin: {}", zx::make_result(result->error_value()));
        return -2;
      }

      if (config.has_drive_strength_ua()) {
        if (result->value()->new_config.has_drive_strength_ua()) {
          printf("Set drive strength to %lu\n", result->value()->new_config.drive_strength_ua());
        } else {
          std::println(stderr, "Driver did not return new drive strength");
        }
      }
      break;
    }
    case GetDriveStrength: {
      // Call Configure() with no arguments to retrieve the current configuration.
      auto result = pin_client->Configure({});
      if (!result.ok()) {
        std::println(stderr, "Call to get pin drive strength failed: {}", result.error());
        return -2;
      }
      if (result->is_error()) {
        std::println(stderr, "Could not get pin drive strength: {}",
                     zx::make_result(result->error_value()));
        return -2;
      }

      if (result->value()->new_config.has_drive_strength_ua()) {
        std::println("Drive strength: {} ua", result->value()->new_config.drive_strength_ua());
      } else {
        std::println(stderr, "Driver did not return drive strength");
      }
      break;
    }
    default:
      std::println(stderr, "Invalid function\n");
      return -1;
  }
  return 0;
}
