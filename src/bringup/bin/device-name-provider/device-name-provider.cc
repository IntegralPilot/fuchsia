// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.boot/cpp/wire.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fdio/namespace.h>
#include <lib/fit/defer.h>
#include <lib/zbi-format/zbi.h>
#include <limits.h>
#include <stdio.h>
#include <string.h>
#include <sys/types.h>
#include <zircon/status.h>

#include <cerrno>

#include "args.h"
#include "name_tokens.h"
#include "src/bringup/bin/device-name-provider/device_name_provider_config.h"
#include "src/bringup/bin/netsvc/netifc-discover.h"

// Copies a word from the wordlist starting at |dest| and then adds |sep| at the end.
// Returns a pointer to the character after the separator.
char* append_word(char* dest, uint16_t num, char sep) {
  const char* word = dictionary[num % TOKEN_DICTIONARY_SIZE];
  memcpy(dest, word, strlen(word));
  dest += strlen(word);
  *dest = sep;
  dest++;
  return dest;
}

void device_id_get_words(const unsigned char mac[6], char out[HOST_NAME_MAX]) {
  char* dest = out;
  dest = append_word(dest, static_cast<uint16_t>(mac[0] | ((mac[4] << 8) & 0xF00)), '-');
  dest = append_word(dest, static_cast<uint16_t>(mac[1] | ((mac[5] << 8) & 0xF00)), '-');
  dest = append_word(dest, static_cast<uint16_t>(mac[2] | ((mac[4] << 4) & 0xF00)), '-');
  dest = append_word(dest, static_cast<uint16_t>(mac[3] | ((mac[5] << 4) & 0xF00)), 0);
}

const char hex_chars[17] = "0123456789abcdef";

// Copies 4 hex characters of hex value of the bits of |num|.
// Then writes |sep| to the character after.
// Returns a pointer to the character after the separator.
char* append_hex(char* dest, uint16_t num, char sep) {
  for (uint8_t i = 0; i < 4; i++) {
    uint16_t left = num >> ((3 - i) * 4);
    *dest = hex_chars[left & 0x0F];
    dest++;
  }
  *dest = sep;
  dest++;
  return dest;
}

#define PREFIX_LEN 9
const char mac_prefix[PREFIX_LEN] = "fuchsia-";

void device_id_get_mac(const unsigned char mac[6], char out[HOST_NAME_MAX]) {
  char* dest = out;
  // Prepend with 'fs-'
  // Prepended with mac_prefix
  for (uint8_t i = 0; i < PREFIX_LEN; i++) {
    dest[i] = mac_prefix[i];
  }
  dest = dest + PREFIX_LEN - 1;
  dest = append_hex(dest, static_cast<uint16_t>((mac[0] << 8) | mac[1]), '-');
  dest = append_hex(dest, static_cast<uint16_t>((mac[2] << 8) | mac[3]), '-');
  dest = append_hex(dest, static_cast<uint16_t>((mac[4] << 8) | mac[5]), 0);
}

void device_id_get(const unsigned char mac[6], char out[HOST_NAME_MAX], uint32_t generation) {
  if (generation == 1) {
    device_id_get_mac(mac, out);
  } else {  // Style 0
    device_id_get_words(mac, out);
  }
}

class DeviceNameProviderServer final : public fidl::WireServer<fuchsia_device::NameProvider> {
  const char* name;
  const size_t size;

 public:
  DeviceNameProviderServer(const char* device_name, size_t size) : name(device_name), size(size) {}
  void GetDeviceName(GetDeviceNameCompleter::Sync& completer) override {
    completer.ReplySuccess(fidl::StringView::FromExternal(name, size));
  }
};

/// Get the MAC address of the primary interface from Boot Items. This will work even
/// if the device isn't configured to enable the interface.
bool get_boot_item_mac(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root, uint32_t iface_idx,
                       bool flip_mac, unsigned char mac[6]) {
  zx::result client_end = component::ConnectAt<fuchsia_boot::Items>(svc_root);
  if (client_end.is_error()) {
    printf("device-name-provider: Could not connect to fuchsia.boot.Items: %s\n",
           client_end.status_string());
    return false;
  }

  auto response = fidl::WireCall(client_end.value())->Get(ZBI_TYPE_DRV_MAC_ADDRESS, iface_idx);
  if (!response.ok()) {
    printf("device-name-provider: Could not get MAC address from Boot Items: %s\n",
           response.status_string());
    return false;
  }

  size_t len = std::min(response->length, 6u);
  auto status = response->payload.read(mac, 0, len);

  if (status != ZX_OK) {
    printf("device-name-provider: Could not read MAC address from Boot Items VMO: %s\n",
           zx_status_get_string(status));
    return false;
  }

  std::fill(mac + len, mac + 6, 0);
  if (flip_mac) {
    mac[5] ^= 1;
  }

  return true;
}

int main(int argc, char** argv) {
  // TODO(https://fxbug.dev/42073486): Remove this once the elf runner no longer
  // fools libc into block-buffering stdout.
  setlinebuf(stdout);

  auto config = device_name_provider_config::Config::TakeFromStartupHandle();
  DeviceNameProviderArgs args;
  const char* errmsg = nullptr;
  int err = ParseArgs(argc, argv, config, &errmsg, &args);
  if (err) {
    printf("device-name-provider: FATAL: ParseArgs(_) = %d; %s\n", err, errmsg);
    return err;
  }

  char device_name[HOST_NAME_MAX];
  std::array<unsigned char, 6> boot_item_mac;
  zx::result svc_root = component::OpenServiceRoot();
  if (svc_root.is_error()) {
    printf("device-name-provider: Could not open service root: %s\n", svc_root.status_string());
  }

  if (!args.nodename.empty()) {
    strlcpy(device_name, args.nodename.c_str(), sizeof(device_name));
  } else if (svc_root.is_ok() &&
             get_boot_item_mac(svc_root.value(), config.boot_item_interface_for_node_name(),
                               config.boot_item_interface_flip_low_mac_bit(),
                               boot_item_mac.data())) {
    device_id_get(boot_item_mac.data(), device_name, args.namegen);
    printf("device-name-provider: generated device name from Boot Items: %s\n", device_name);
  } else {
    zx::result status = netifc_discover(args.devdir, args.interface);
    if (status.is_error()) {
      strlcpy(device_name, fuchsia_device::wire::kDefaultDeviceName, sizeof(device_name));
      printf("device-name-provider: using default name \"%s\": netifc_discover(\"%s\", ...) = %s\n",
             device_name, args.devdir.c_str(), status.status_string());
    } else {
      const NetdeviceInterface& interface = status.value();
      device_id_get(interface.mac.x, device_name, args.namegen);
      printf("device-name-provider: generated device name from discovered interface: %s\n",
             device_name);
    }
  }

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  async_dispatcher_t* dispatcher = loop.dispatcher();
  if (dispatcher == nullptr) {
    printf("device-name-provider: FATAL: loop.dispatcher() = nullptr\n");
    return -1;
  }

  component::OutgoingDirectory outgoing(dispatcher);
  if (zx::result result = outgoing.ServeFromStartupInfo(); result.is_error()) {
    printf("device-name-provider: FATAL: outgoing.ServeFromStartupInfo() = %s\n",
           result.status_string());
    return -1;
  }

  DeviceNameProviderServer server(device_name, strnlen(device_name, sizeof(device_name)));

  if (zx::result result = outgoing.AddUnmanagedProtocol<fuchsia_device::NameProvider>(
          [dispatcher, server](fidl::ServerEnd<fuchsia_device::NameProvider> server_end) mutable {
            fidl::BindServer(dispatcher, std::move(server_end), &server);
          });
      result.is_error()) {
    printf("device-name-provider: FATAL: outgoing.AddUnmanagedProtocol = %s\n",
           result.status_string());
    return -1;
  }

  zx_status_t status = loop.Run();
  printf("device-name-provider: loop.Run() = %s\n", zx_status_get_string(status));
  return status;
}
