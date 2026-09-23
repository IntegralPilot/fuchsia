// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <fcntl.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/component/incoming/cpp/clone.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fit/defer.h>
#include <lib/zx/channel.h>
#include <lib/zx/clock.h>
#include <lib/zx/fifo.h>
#include <lib/zx/vmo.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <zircon/syscalls.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <ramdevice-client/ramdisk.h>

#include "src/storage/lib/block_client/cpp/client.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"

static uint64_t number(const char* str) {
  char* end;
  uint64_t n = strtoull(str, &end, 10);

  uint64_t m = 1;
  switch (*end) {
    case 'G':
    case 'g':
      m = 1024 * 1024 * 1024;
      break;
    case 'M':
    case 'm':
      m = 1024 * 1024;
      break;
    case 'K':
    case 'k':
      m = 1024;
      break;
  }
  return m * n;
}

static zx::duration iotime_posix(int is_read, fbl::unique_fd fd, size_t total_io_size,
                                 size_t buffer_size) {
  std::unique_ptr<unsigned char[]> buffer(new unsigned char[buffer_size]);
  if (buffer.get() == nullptr) {
    fprintf(stderr, "error: out of memory\n");
    return zx::duration::infinite();
  }

  const zx::time t0 = zx::clock::get_monotonic();
  size_t bytes_remaining = total_io_size;
  const char* fn_name = is_read ? "read" : "write";
  while (bytes_remaining > 0) {
    size_t transfer_size = std::min(buffer_size, bytes_remaining);
    ssize_t r = is_read ? read(fd.get(), buffer.get(), transfer_size)
                        : write(fd.get(), buffer.get(), transfer_size);
    if (r < 0) {
      fprintf(stderr, "error: %s() error %d\n", fn_name, errno);
      return zx::duration::infinite();
    }
    if (static_cast<size_t>(r) != transfer_size) {
      fprintf(stderr, "error: %s() %zu of %zu bytes processed\n", fn_name, r, transfer_size);
      return zx::duration::infinite();
    }
    bytes_remaining -= transfer_size;
  }
  const zx::time t1 = zx::clock::get_monotonic();

  return t1 - t0;
}

static zx::duration iotime_block(int is_read,
                                 fidl::UnownedClientEnd<fuchsia_storage_block::Block> block_client,
                                 size_t total_io_size, size_t buffer_size) {
  if ((total_io_size % 4096) || (buffer_size % 4096)) {
    fprintf(stderr, "error: total_io_size and buffer_size must be multiples of 4K\n");
    return zx::duration::infinite();
  }

  std::unique_ptr<unsigned char[]> buffer(new unsigned char[buffer_size]);
  if (buffer.get() == nullptr) {
    fprintf(stderr, "error: out of memory\n");
    return zx::duration::infinite();
  }

  size_t bytes_remaining = total_io_size;
  const zx::time t0 = zx::clock::get_monotonic();
  while (bytes_remaining > 0) {
    size_t transfer_size = std::min(buffer_size, bytes_remaining);

    if (is_read) {
      zx_status_t status = block_client::SingleReadBytes(block_client, buffer.get(), transfer_size,
                                                         total_io_size - bytes_remaining);
      if (status != ZX_OK) {
        fprintf(stderr, "error: failed SingleReadBytes() call: %s\n", zx_status_get_string(status));
        return zx::duration::infinite();
      }
    } else {
      zx_status_t status = block_client::SingleWriteBytes(block_client, buffer.get(), transfer_size,
                                                          total_io_size - bytes_remaining);
      if (status != ZX_OK) {
        fprintf(stderr, "error: failed SingleWriteBytes() call: %s\n",
                zx_status_get_string(status));
        return zx::duration::infinite();
      }
    }

    bytes_remaining -= transfer_size;
  }
  const zx::time t1 = zx::clock::get_monotonic();

  return t1 - t0;
}

static zx::duration iotime_fifo(const char* dev, int is_read, fbl::unique_fd fd,
                                size_t total_io_size, size_t buffer_size) {
  fdio_cpp::FdioCaller disk_connection(std::move(fd));
  zx::result channel = disk_connection.take_as<fuchsia_storage_block::Block>();
  if (channel.is_error()) {
    fprintf(stderr, "error: cannot take volume channel for '%s': %s\n", dev,
            channel.status_string());
    return zx::duration::infinite();
  }
  zx::result block_device = block_client::RemoteBlockDevice::Create(std::move(channel.value()));
  if (block_device.is_error()) {
    fprintf(stderr, "error: cannot create remote block device for '%s': %s\n", dev,
            block_device.status_string());
    return zx::duration::infinite();
  }

  fuchsia_storage_block::wire::BlockInfo info;
  if (zx_status_t status = block_device->BlockGetInfo(&info); status != ZX_OK) {
    fprintf(stderr, "error: failed BlockGetInfo() call for '%s':%s\n", dev,
            zx_status_get_string(status));
    return zx::duration::infinite();
  }

  zx::vmo vmo;
  if (zx_status_t status = zx::vmo::create(buffer_size, 0, &vmo); status != ZX_OK) {
    fprintf(stderr, "error: out of memory: %s\n", zx_status_get_string(status));
    return zx::duration::infinite();
  }

  storage::Vmoid vmoid;
  if (zx_status_t status = block_device->BlockAttachVmo(vmo, &vmoid); status != ZX_OK) {
    fprintf(stderr, "error: cannot attach vmo for '%s':%s\n", dev, zx_status_get_string(status));
    return zx::duration::infinite();
  }

  auto cleanup = fit::defer([&]() { block_device->BlockDetachVmo(std::move(vmoid)); });

  const uint8_t opcode = is_read ? BLOCK_OPCODE_READ : BLOCK_OPCODE_WRITE;
  BlockFifoRequest request = {
      .command = {.opcode = opcode, .flags = 0},
      .reqid = 0,
      .group = 0,
      .vmoid = vmoid.get(),
      .vmo_offset = 0,
  };

  size_t bytes_remaining = total_io_size;
  const zx::time t0 = zx::clock::get_monotonic();
  while (bytes_remaining > 0) {
    size_t transfer_size = std::min(buffer_size, bytes_remaining);
    request.length = static_cast<uint32_t>(transfer_size / info.block_size);
    request.dev_offset = (total_io_size - bytes_remaining) / info.block_size;
    if (zx_status_t status = block_device->FifoTransaction(&request, 1); status != ZX_OK) {
      fprintf(stderr, "error: block_fifo_txn error %s\n", zx_status_get_string(status));
      return zx::duration::infinite();
    }
    bytes_remaining -= transfer_size;
  }
  const zx::time t1 = zx::clock::get_monotonic();
  return t1 - t0;
}

static int usage() {
  fprintf(stderr,
          "usage: iotime <read|write> <posix|block|fifo> <device|--ramdisk> <bytes> <bufsize>\n\n"
          "        <bytes> and <bufsize> must be a multiple of 4k for block mode\n"
          "        --ramdisk only supported for block mode\n");
  return -1;
}

int main(int argc, char** argv) {
  if (argc != 6) {
    return usage();
  }

  const std::string io_type(argv[1]);
  const std::string mode(argv[2]);
  const std::string device(argv[3]);
  const size_t total_io_size = number(argv[4]);
  const size_t buffer_size = number(argv[5]);

  const bool is_read = io_type == "read";
  if (!is_read && io_type != "write") {
    fprintf(stderr, "io type can only be 'read' or 'write'. got: %s\n", io_type.c_str());
    return usage();
  }

  ramdevice_client::Ramdisk ramdisk;
  fbl::unique_fd fd;
  zx::duration io_duration;
  if (device == "--ramdisk") {
    if (mode != "block") {
      fprintf(stderr, "ramdisk only supported for block\n");
      return EXIT_FAILURE;
    }
    constexpr size_t kBlockSizeBytes = 512;
    zx::result result =
        ramdevice_client::Ramdisk::Create(kBlockSizeBytes, total_io_size / kBlockSizeBytes);
    if (result.is_error()) {
      fprintf(stderr, "error: cannot create %zu-byte ramdisk: %s\n", total_io_size,
              result.status_string());
      return EXIT_FAILURE;
    }
    ramdisk = std::move(result.value());
    zx::result block = ramdisk.ConnectBlock();
    if (block.is_error()) {
      fprintf(stderr, "error: cannot get ramdisk channel: %s\n", block.status_string());
      return EXIT_FAILURE;
    }
    io_duration = iotime_block(is_read, block->borrow(), total_io_size, buffer_size);
  } else {
    fd.reset(open(device.c_str(), is_read ? O_RDONLY : O_WRONLY));
    if (fd.get() < 0) {
      fprintf(stderr, "error: cannot open '%s': %s\n", device.c_str(), strerror(errno));
      return EXIT_FAILURE;
    }
    if (mode == "posix") {
      io_duration = iotime_posix(is_read, std::move(fd), total_io_size, buffer_size);
    } else if (mode == "block") {
      fdio_cpp::UnownedFdioCaller disk_connection(fd);
      fidl::UnownedClientEnd channel = disk_connection.borrow_as<fuchsia_storage_block::Block>();
      io_duration = iotime_block(is_read, channel, total_io_size, buffer_size);
    } else if (mode == "fifo") {
      io_duration = iotime_fifo(device.c_str(), is_read, std::move(fd), total_io_size, buffer_size);
    } else {
      fprintf(stderr, "error: unsupported mode '%s'\n", mode.c_str());
      return EXIT_FAILURE;
    }
  }

  if (io_duration == zx::duration::infinite()) {
    return EXIT_FAILURE;
  }
  double s = (static_cast<double>(io_duration.to_nsecs())) / (static_cast<double>(1000000000));
  double rate = (static_cast<double>(total_io_size)) / s;
  const char* unit = "B";
  if (rate > 1024 * 1024) {
    unit = "MB";
    rate /= 1024 * 1024;
  } else if (rate > 1024) {
    unit = "KB";
    rate /= 1024;
  }
  fprintf(stderr, "%s %zu bytes (%zu chunks) in %zu ns: %g %s/s\n", io_type.c_str(), total_io_size,
          buffer_size, io_duration.to_nsecs(), rate, unit);
  return EXIT_SUCCESS;
}
