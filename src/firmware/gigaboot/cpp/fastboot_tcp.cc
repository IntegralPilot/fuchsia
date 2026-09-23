
// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "fastboot_tcp.h"

#include <lib/fit/result.h>
#include <log.h>
#include <stdio.h>
#include <stdlib.h>

#include "efi/types.h"
#include "fastboot.h"
#include "gigaboot/src/tcp.h"
#include "lib/zx/time.h"
#include "mdns.h"
#include "phys/efi/main.h"
#include "zircon_boot_ops.h"

namespace gigaboot {
namespace {
constexpr uint16_t kFbServerPort = 5554;

constexpr size_t kPageSize = 4096;
constexpr size_t kDownloadBufferSize = 1ULL * 1024 * 1024 * 1024;  // 1GB
constexpr size_t kDownloadBufferPageCount = kDownloadBufferSize / kPageSize;

zx::result<> TcpInitialize(tcp6_socket &fb_tcp_socket, const MacAddr &mac) {
  if (fb_tcp_socket.binding_protocol) {
    return zx::ok();
  }

  // Use the unspecified address (::) so UEFI's TCP6 driver auto-selects a local address.
  efi_ipv6_addr unspecified_addr = {};

  while (true) {
    if (tcp6_open(&fb_tcp_socket, gEfiSystemTable->BootServices, mac.data(), &unspecified_addr,
                  kFbServerPort) == TCP6_RESULT_SUCCESS) {
      printf("Fastboot TCP is ready\n");
      return zx::ok();
    }
  }
}

}  // namespace

class TcpTransport : public TcpTransportInterface {
 public:
  TcpTransport(tcp6_socket &fb_tcp_socket)
      : fb_tcp_socket_(fb_tcp_socket), io_timer_(gEfiSystemTable) {}

  bool Read(void *out, size_t size) override {
    if (io_timer_.SetTimer(TimerRelative, zx::sec(30)).is_error()) {
      ELOG("Failed to set read timer");
      return false;
    }

    tcp6_result res = TCP6_RESULT_PENDING;
    ZX_ASSERT(size < UINT32_MAX);
    // Block until complete or error.
    while (res == TCP6_RESULT_PENDING && io_timer_.CheckTimer() == Timer::Status::kWaiting) {
      res = tcp6_read(&fb_tcp_socket_, out, static_cast<uint32_t>(size));
    }
    return res == TCP6_RESULT_SUCCESS;
  }

  bool Write(const void *data, size_t size) override {
    if (io_timer_.SetTimer(TimerRelative, zx::sec(30)).is_error()) {
      ELOG("Failed to set read timer");
      return false;
    }

    tcp6_result res = TCP6_RESULT_PENDING;
    ZX_ASSERT(size < UINT32_MAX);
    // Block until complete or error.
    while (res == TCP6_RESULT_PENDING && io_timer_.CheckTimer() == Timer::Status::kWaiting) {
      res = tcp6_write(&fb_tcp_socket_, data, static_cast<uint32_t>(size));
    }
    return res == TCP6_RESULT_SUCCESS;
  }

 private:
  tcp6_socket &fb_tcp_socket_;
  Timer io_timer_;
};

zx::result<> FastbootTcpMain() {
  // Once |fb_tcp_socket| is initialized, parts of the struct are under control of the TCP driver
  // and may be modified asynchronously. This means we cannot return until we successfully close
  // the socket, or else we're likely to get stack corruption in the bytes that used to be part of
  // |fb_tcp_socket|.
  //
  // We could potentially move this to the heap if we want to be able to return without closing the
  // socket, but it doesn't seem necessary at this point; fastboot isn't part of the standard boot
  // flow so it's probably OK to just abort() on TCP failure rather than trying to push through.

  // EthernetAgent will initialize on the active NIC. If there are multiple active NICs, only the
  // first will be selected.
  auto res = EthernetAgent::Create();
  if (res.is_error()) {
    printf("Error creating ethernet agent: %s\n", EfiStatusToString(res.error_value()));
    return zx::error(ZX_ERR_INTERNAL);
  }
  EthernetAgent eth_agent = std::move(res.value());

  // Use the active NIC's MAC address to determine which NIC to open the fastboot server on.
  tcp6_socket fb_tcp_socket = {};
  auto init_res = TcpInitialize(fb_tcp_socket, eth_agent.Addr());
  if (init_res.is_error()) {
    return zx::error(ZX_ERR_INTERNAL);
  }

  TcpTransport transport(fb_tcp_socket);
  ZirconBootOps zb_ops = gigaboot::GetZirconBootOps();

  // Due to the size of this allocation, use AllocatePages directly.
  // AllocatePool is not designed for allocations this size. Due to internal book keeping in some
  // implementations it may take upwards of 10 seconds to allocate a huge buffer from the pool.
  // operator new[] suffers the same issue as it delegates to AllocatePool.
  efi_physical_addr download_buffer;
  efi_status status = gEfiSystemTable->BootServices->AllocatePages(
      AllocateAnyPages, EfiLoaderData, kDownloadBufferPageCount, &download_buffer);
  if (status != EFI_SUCCESS) {
    printf("Failed to allocate download buffer\n");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  Fastboot fastboot({reinterpret_cast<uint8_t *>(download_buffer), kDownloadBufferSize}, zb_ops);
  MdnsAgent mdns_agent(eth_agent, gEfiSystemTable);
  while (true) {
    if (auto poll = mdns_agent.Poll(); poll.is_error()) {
      printf("mDNS poll error: %s\n", EfiStatusToString(poll.error_value()));
    }

    tcp6_result result = tcp6_accept(&fb_tcp_socket);

    if (result == TCP6_RESULT_SUCCESS) {
      printf("Receive client connection\n");
      FastbootTcpSession(transport, fastboot);
      if (fastboot.IsContinue()) {
        break;
      }

      printf("Disconnecting tcp6...");
      auto disconnect_res = tcp6_disconnect(&fb_tcp_socket);
      while (disconnect_res == TCP6_RESULT_PENDING) {
        disconnect_res = tcp6_disconnect(&fb_tcp_socket);
      }

      if (disconnect_res != TCP6_RESULT_SUCCESS) {
        // Failed to disconnect; socket is in an unknown state and cannot be recovered. Do not
        // return; see notes at the beginning of function.
        printf("FATAL: failed to disconnect socket, %d\n", disconnect_res);
        abort();
      }
      printf("disconnected\n");
    }
  }

  gEfiSystemTable->BootServices->FreePages(download_buffer, kDownloadBufferPageCount);
  while (true) {
    auto result = tcp6_close(&fb_tcp_socket);
    if (result == TCP6_RESULT_SUCCESS) {
      break;
    } else if (result == TCP6_RESULT_PENDING) {
      continue;
    } else {
      // Failed to close; socket is in an unknown state and cannot be recovered. Do not return; see
      // notes at the beginning of function.
      printf("FATAL: failed to close socket, %d\n", result);
      abort();
    }
  }

  return zx::ok();
}

}  // namespace gigaboot
