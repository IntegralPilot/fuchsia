// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <ctype.h>
#include <errno.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/spi/spi.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <zircon/status.h>

#include <string>

void usage(char* prog) {
  fprintf(stderr, "usage:\n");
  fprintf(stderr, "    %s DEVICE r LENGTH\n", prog);
  fprintf(stderr, "    %s DEVICE w BYTES ...\n", prog);
  fprintf(stderr, "    %s DEVICE x BYTES ...\n", prog);
}

void convert_args(char** argv, size_t length, uint8_t* buffer) {
  for (size_t i = 0; i < length; i++) {
    buffer[i] = static_cast<uint8_t>(strtoul(argv[i], nullptr, 0));
  }
}

void print_buffer(uint8_t* buffer, size_t length) {
  char ascii[16];
  char* a = ascii;
  for (size_t i = 0; i < length; i++) {
    if (i % 16 == 0) {
      printf("%04zx: ", i);
      a = ascii;
    }

    printf("%02x ", buffer[i]);
    if (isprint(buffer[i])) {
      *a++ = static_cast<char>(buffer[i]);
    } else {
      *a++ = '.';
    }

    if (i % 16 == 15) {
      printf("|%.16s|\n", ascii);
    } else if (i % 8 == 7) {
      printf(" ");
    }
  }

  int rem = static_cast<int>(length) % 16;
  if (rem != 0) {
    int spaces = (16 - rem) * 3;
    if (rem < 8) {
      spaces++;
    }
    printf("%*s|%.*s|\n", spaces, "", rem, ascii);
  }
}

int main(int argc, char** argv) {
  if (argc < 4) {
    usage(argv[0]);
    return -1;
  }

  zx::result controller = component::Connect<fuchsia_hardware_spi::Controller>(argv[1]);
  if (controller.is_error()) {
    fprintf(stderr, "component::Connect(%s): %s\n", argv[1], controller.status_string());
    return -1;
  }
  auto [device, server] = fidl::Endpoints<fuchsia_hardware_spi::Device>::Create();

  const fidl::Status result = fidl::WireCall(controller.value())->OpenSession(std::move(server));
  if (!result.ok()) {
    fprintf(stderr, "(%s)->OpenSession(): %s\n", argv[1], result.status_string());
    return -1;
  }

  switch (argv[2][0]) {
    case 'r': {
      uint32_t length = static_cast<uint32_t>(std::stoul(argv[3], nullptr, 0));
      uint8_t buffer[length];
      if (zx_status_t status = spilib_receive(device, buffer, length); status != ZX_OK) {
        fprintf(stderr, "error: spilib_receive failed: %s\n", zx_status_get_string(status));
        return -1;
      }
      print_buffer(buffer, length);
      break;
    }
    case 'w': {
      size_t length = argc - 3;
      uint8_t buffer[length];
      convert_args(&argv[3], length, buffer);
      if (zx_status_t status = spilib_transmit(device, buffer, length); status != ZX_OK) {
        fprintf(stderr, "error: spilib_transmit failed: %s\n", zx_status_get_string(status));
        return -1;
      }
      break;
    }
    case 'x': {
      size_t length = argc - 3;
      uint8_t send[length];
      uint8_t recv[length];
      convert_args(&argv[3], length, send);
      if (zx_status_t status = spilib_exchange(device, send, recv, length); status != ZX_OK) {
        fprintf(stderr, "error: spilib_exchange failed: %s\n", zx_status_get_string(status));
        return -1;
      }
      print_buffer(recv, length);
      break;
    }
    default:
      fprintf(stderr, "%c: unrecognized command\n", argv[2][0]);
      usage(argv[0]);
      return -1;
  }

  return 0;
}
