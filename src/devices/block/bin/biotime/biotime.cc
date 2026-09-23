// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fit/defer.h>
#include <lib/zircon-internal/xorshiftrand.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <threads.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <semaphore>

#include <perftest/results.h>

#include "src/storage/lib/block_protocol/block-fifo.h"

namespace {

constexpr uint64_t kKibi = 1024;
constexpr uint64_t kKilo = 1000;

uint64_t HumanReadableStringToNumber(const char* string, bool use_kibi) {
  char* end;
  const uint64_t number = strtoull(string, &end, 10);
  const uint64_t thousand = use_kibi ? kKibi : kKilo;

  uint64_t multiplier = 1;
  switch (*end) {
    case 'G':
    case 'g':
      multiplier = thousand * thousand * thousand;
      break;
    case 'M':
    case 'm':
      multiplier = thousand * thousand;
      break;
    case 'K':
    case 'k':
      multiplier = thousand;
      break;
    default:
      // TODO(hanbinyoon): Handle error.
      break;
  }
  return multiplier * number;
}

void PrintRate(uint64_t count, uint64_t nanos, const char* unit, bool use_kibi) {
  const double seconds =
      (static_cast<double>(nanos)) / (static_cast<double>(kKilo * kKilo * kKilo));
  double rate = (static_cast<double>(count)) / seconds;
  const double thousand = use_kibi ? kKibi : kKilo;

  const char* order = "";
  if (rate > thousand * thousand) {
    order = use_kibi ? "Mi" : "M";
    rate /= thousand * thousand;
  } else if (rate > thousand) {
    order = use_kibi ? "Ki" : "K";
    rate /= thousand;
  }
  fprintf(stderr, "%g %s%s/s\n", rate, order, unit);
}

struct BlockDevice {
  zx::vmo vmo;
  zx::fifo fifo;
  fidl::ClientEnd<fuchsia_storage_block::Session> session;
  reqid_t reqid;
  fuchsia_storage_block::wire::VmoId vmoid;
  size_t buffer_size;
  fuchsia_storage_block::wire::BlockInfo info;
};

zx::result<BlockDevice> OpenBlockDevice(const char* dev, size_t buffer_size) {
  zx::result block = component::Connect<fuchsia_storage_block::Block>(dev);
  if (block.is_error()) {
    fprintf(stderr, "error: cannot open '%s': %s\n", dev, block.status_string());
    return block.take_error();
  }
  BlockDevice block_device = {
      .buffer_size = buffer_size,
  };

  {
    const fidl::WireResult result = fidl::WireCall(block.value())->GetInfo();
    if (!result.ok()) {
      fprintf(stderr, "error: cannot get block device info for '%s': %s\n", dev,
              result.FormatDescription().c_str());
      return zx::error(result.status());
    }
    const fit::result response = result.value();
    if (response.is_error()) {
      fprintf(stderr, "error: cannot get block device info for '%s':%s\n", dev,
              zx_status_get_string(response.error_value()));
      return zx::error(response.error_value());
    }
    block_device.info = response.value()->info;
  }

  {
    zx::result server = fidl::CreateEndpoints(&block_device.session);
    if (server.is_error()) {
      fprintf(stderr, "error: cannot create server for '%s': %s\n", dev, server.status_string());
      return zx::error(server.status_value());
    }
    if (fidl::Status result = fidl::WireCall(block.value())->OpenSession(std::move(server.value()));
        !result.ok()) {
      fprintf(stderr, "error: cannot open session for '%s': %s\n", dev,
              result.FormatDescription().c_str());
      return zx::error(result.status());
    }
  }

  {
    const fidl::WireResult result = fidl::WireCall(block_device.session)->GetFifo();
    if (!result.ok()) {
      fprintf(stderr, "error: cannot get fifo for '%s':%s\n", dev,
              result.FormatDescription().c_str());
      return zx::error(result.status());
    }
    const fit::result response = result.value();
    if (response.is_error()) {
      fprintf(stderr, "error: cannot get fifo for '%s':%s\n", dev,
              zx_status_get_string(response.error_value()));
      return zx::error(response.error_value());
    }
    block_device.fifo = std::move(response.value()->fifo);
  }

  if (zx_status_t status = zx::vmo::create(buffer_size, 0, &block_device.vmo); status != ZX_OK) {
    fprintf(stderr, "error: out of memory: %s\n", zx_status_get_string(status));
    return zx::error(status);
  }

  zx::vmo dup;
  if (zx_status_t status = block_device.vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &dup);
      status != ZX_OK) {
    fprintf(stderr, "error: cannot duplicate handle: %s\n", zx_status_get_string(status));
    return zx::error(status);
  }

  {
    const fidl::WireResult result = fidl::WireCall(block_device.session)->AttachVmo(std::move(dup));
    if (!result.ok()) {
      fprintf(stderr, "error: cannot attach vmo for '%s':%s\n", dev,
              result.FormatDescription().c_str());
      return zx::error(result.status());
    }
    const fit::result response = result.value();
    if (response.is_error()) {
      fprintf(stderr, "error: cannot attach vmo for '%s':%s\n", dev,
              zx_status_get_string(response.error_value()));
      return zx::error(response.error_value());
    }
    block_device.vmoid = response.value()->vmoid;
  }

  return zx::ok(std::move(block_device));
}

struct BioReadWriteArgs {
  BlockDevice& block_device;
  size_t total_xfer_ops;
  size_t xfer_size_bytes;
  uint64_t random_seed;
  bool is_write;
  bool is_linear_access;

  mutable std::counting_semaphore<> max_pending_ops;
};

zx_status_t BioReadWrite(const BioReadWriteArgs& rw_args, zx_duration_t* xfer_duration) {
  const zx_time_t before_xfer = zx_clock_get_monotonic();

  std::thread t([&rw_args]() {
    size_t total_xfer_ops = rw_args.total_xfer_ops;
    const size_t xfer_size_bytes = rw_args.xfer_size_bytes;
    const size_t block_size = rw_args.block_device.info.block_size;
    const size_t max_block_offset = ((total_xfer_ops - 1) * xfer_size_bytes) / block_size;

    reqid_t next_reqid(0);
    size_t vmo_offset = 0;
    size_t linear_dev_offset = 0;
    rand64_t r64 = RAND63SEED(rw_args.random_seed);
    zx::fifo& fifo = rw_args.block_device.fifo;

    while (total_xfer_ops > 0) {
      rw_args.max_pending_ops.acquire();

      BlockFifoRequest req = {
          .command =
              {
                  .opcode = static_cast<uint8_t>(rw_args.is_write ? BLOCK_OPCODE_WRITE
                                                                  : BLOCK_OPCODE_READ),
                  .flags = 0,
              },
          .reqid = next_reqid++,
          .vmoid = rw_args.block_device.vmoid.id,
          .length = static_cast<uint32_t>(xfer_size_bytes),
          .vmo_offset = vmo_offset,
      };

      if (rw_args.is_linear_access) {
        req.dev_offset = linear_dev_offset;
        linear_dev_offset += xfer_size_bytes;
      } else {
        req.dev_offset = (rand64(&r64) % max_block_offset) * block_size;
      }
      vmo_offset += xfer_size_bytes;
      if ((vmo_offset + xfer_size_bytes) > rw_args.block_device.buffer_size) {
        vmo_offset = 0;
      }

      // TODO(hanbinyoon): Consider using units of block_size above instead.
      req.length /= static_cast<uint32_t>(block_size);
      req.dev_offset /= block_size;
      req.vmo_offset /= block_size;

#if 0
        fprintf(stderr, "IO tid=%u vid=%u op=%x len=%zu vof=%zu dof=%zu\n",
                req.reqid, req.vmoid.id, req.opcode, req.length, req.vmo_offset, req.dev_offset);
#endif
      if (zx_status_t status = fifo.write(sizeof(req), &req, 1, nullptr); status != ZX_OK) {
        if (status == ZX_ERR_SHOULD_WAIT) {
          if (zx_status_t status = fifo.wait_one(ZX_FIFO_WRITABLE | ZX_FIFO_PEER_CLOSED,
                                                 zx::time::infinite(), nullptr);
              status != ZX_OK) {
            fprintf(stderr, "failed waiting for fifo: %s\n", zx_status_get_string(status));
            fifo.reset();
            return -1;
          }
          continue;
        }
        fprintf(stderr, "error: failed writing to fifo: %s\n", zx_status_get_string(status));
        fifo.reset();
        return -1;
      }

      total_xfer_ops--;
    }
    return 0;
  });

  zx::fifo& fifo = rw_args.block_device.fifo;
  auto cleanup = fit::defer([&fifo, &t]() {
    fifo.reset();
    t.join();
  });

  size_t total_xfer_ops = rw_args.total_xfer_ops;
  while (total_xfer_ops > 0) {
    BlockFifoResponse resp;
    if (zx_status_t status = fifo.read(sizeof(resp), &resp, 1, nullptr); status != ZX_OK) {
      if (status == ZX_ERR_SHOULD_WAIT) {
        if (zx_status_t status = fifo.wait_one(ZX_FIFO_READABLE | ZX_FIFO_PEER_CLOSED,
                                               zx::time::infinite(), nullptr);
            status != ZX_OK) {
          fprintf(stderr, "failed waiting for fifo: %s\n", zx_status_get_string(status));
          return status;
        }
        continue;
      }
      fprintf(stderr, "error: failed reading fifo: %s\n", zx_status_get_string(status));
      return status;
    }
    if (resp.status != ZX_OK) {
      fprintf(stderr, "error: io txn failed %s (%zu remaining)\n",
              zx_status_get_string(resp.status), total_xfer_ops);
      return resp.status;
    }
    total_xfer_ops--;
    rw_args.max_pending_ops.release();
  }

  cleanup.cancel();

  const zx_time_t after_xfer = zx_clock_get_monotonic();

  fprintf(stderr, "waiting for thread to exit...\n");
  t.join();

  *xfer_duration = zx_time_sub_time(after_xfer, before_xfer);
  return ZX_OK;
}

void usage() {
  fprintf(stderr,
          "usage: biotime <option>* <device>\n"
          "\n"
          "args:  -bs <num>     transfer block size (multiple of 4K, default is 32K)\n"
          "       -total-bytes-to-transfer <num>  total amount to read or write (per\n"
          "                                       iteration, default is entire device)\n"
          "       -iter <num>   total number of iterations (0 stands for infinite, default\n"
          "                     is 1). results are reported for each iteration\n"
          "       -mo <num>     maximum outstanding ops (1..128, default is 128)\n"
          "       -read         test reading from the block device (default)\n"
          "       -write        test writing to the block device\n"
          "       -live-dangerously  required if using \"-write\"\n"
          "       -linear       transfers to linearly increasing block addresses (default)\n"
          "       -random       transfers to random addresses across a span of\n"
          "                     -total-bytes-to-transfer bytes\n"
          "       -output-file <filename>  destination file for writing results in JSON\n"
          "                                format\n");
}

#define needparam()                                                      \
  do {                                                                   \
    argc--;                                                              \
    argv++;                                                              \
    if (argc == 0) {                                                     \
      fprintf(stderr, "error: option %s needs a parameter\n", argv[-1]); \
      return -1;                                                         \
    }                                                                    \
  } while (0)
#define nextarg() \
  do {            \
    argc--;       \
    argv++;       \
  } while (0)
#define error(x...)     \
  do {                  \
    fprintf(stderr, x); \
    usage();            \
    return -1;          \
  } while (0)

}  // namespace

int main(int argc, char** argv) {
  bool live_dangerously = false;
  std::optional<bool> opt_is_write;
  std::optional<bool> opt_is_linear_access;
  int opt_max_pending_ops = 128;
  size_t opt_xfer_size_bytes = 32768;
  uint64_t opt_num_iter = 1;
  bool loop_forever = false;

  const char* output_file = nullptr;

  size_t total_xfer_bytes = 0;

  nextarg();
  while (argc > 0) {
    if (argv[0][0] != '-') {
      break;
    }
    if (!strcmp(argv[0], "-bs")) {
      needparam();
      opt_xfer_size_bytes = HumanReadableStringToNumber(argv[0], /*use_kibi=*/true);
      if ((opt_xfer_size_bytes == 0) || (opt_xfer_size_bytes % 4096)) {
        error("error: block size must be multiple of 4K\n");
      }
    } else if (!strcmp(argv[0], "-total-bytes-to-transfer")) {
      needparam();
      total_xfer_bytes = HumanReadableStringToNumber(argv[0], /*use_kibi=*/true);
    } else if (!strcmp(argv[0], "-mo")) {
      needparam();
      size_t n = HumanReadableStringToNumber(argv[0], /*use_kibi=*/false);
      if ((n < 1) || (n > 128)) {
        error("error: max pending must be between 1 and 128\n");
      }
      opt_max_pending_ops = static_cast<int>(n);
    } else if (!strcmp(argv[0], "-read")) {
      if (opt_is_write.has_value() && opt_is_write.value()) {
        error("error: cannot specify both \"-read\" and \"-write\"\n");
      }
      opt_is_write = false;
    } else if (!strcmp(argv[0], "-write")) {
      if (opt_is_write.has_value() && !opt_is_write.value()) {
        error("error: cannot specify both \"-read\" and \"-write\"\n");
      }
      opt_is_write = true;
    } else if (!strcmp(argv[0], "-live-dangerously")) {
      live_dangerously = true;
    } else if (!strcmp(argv[0], "-linear")) {
      if (opt_is_linear_access.has_value() && !opt_is_linear_access.value()) {
        error("error: cannot specify both \"-linear\" and \"-random\"\n");
      }
      opt_is_linear_access = true;
    } else if (!strcmp(argv[0], "-random")) {
      if (opt_is_linear_access.has_value() && opt_is_linear_access.value()) {
        error("error: cannot specify both \"-linear\" and \"-random\"\n");
      }
      opt_is_linear_access = false;
    } else if (!strcmp(argv[0], "-output-file")) {
      needparam();
      output_file = argv[0];
    } else if (!strcmp(argv[0], "-h")) {
      usage();
      return 0;
    } else if (!strcmp(argv[0], "-iter")) {
      needparam();
      opt_num_iter = strtoull(argv[0], nullptr, 10);
      if (opt_num_iter == 0)
        loop_forever = true;
    } else {
      error("error: unknown option: %s\n", argv[0]);
    }
    nextarg();
  }
  if (argc == 0) {
    error("error: no device specified\n");
  }
  if (argc > 1) {
    error("error: unexpected arguments\n");
  }
  if (opt_is_write.has_value() && opt_is_write.value() && !live_dangerously) {
    error(
        "error: the option \"-live-dangerously\" is required when using"
        " \"-write\"\n");
  }
  const char* device_filename = argv[0];

  do {
    const size_t buffer_size = opt_max_pending_ops * opt_xfer_size_bytes;
    zx::result dev = OpenBlockDevice(device_filename, buffer_size);
    if (dev.is_error()) {
      fprintf(stderr, "error: cannot open '%s': %s\n", device_filename, dev.status_string());
      return -1;
    }
    BioReadWriteArgs rw_args = {
        .block_device = dev.value(),
        .xfer_size_bytes = opt_xfer_size_bytes,
        .random_seed = 7891263897612ULL,
        .is_write = opt_is_write.has_value() ? opt_is_write.value() : false,
        .is_linear_access = opt_is_linear_access.has_value() ? opt_is_linear_access.value() : true,
        .max_pending_ops = std::counting_semaphore<>(opt_max_pending_ops),
    };

    const size_t device_size_bytes =
        rw_args.block_device.info.block_count * rw_args.block_device.info.block_size;

    // default to entire device
    if ((total_xfer_bytes == 0) || (total_xfer_bytes > device_size_bytes)) {
      total_xfer_bytes = device_size_bytes;
    }
    rw_args.total_xfer_ops = total_xfer_bytes / rw_args.xfer_size_bytes;

    zx_duration_t xfer_duration = 0;
    if (zx_status_t status = BioReadWrite(rw_args, &xfer_duration); status != ZX_OK) {
      fprintf(stderr, "error: BioReadWrite on '%s': %s\n", device_filename,
              zx_status_get_string(status));
      return -1;
    }

    fprintf(stderr, "%zu bytes in %zu ns: ", total_xfer_bytes, xfer_duration);
    PrintRate(total_xfer_bytes, xfer_duration, "B", /*use_kibi=*/true);
    fprintf(stderr, "%zu ops in %zu ns: ", rw_args.total_xfer_ops, xfer_duration);
    PrintRate(rw_args.total_xfer_ops, xfer_duration, "ops", /*use_kibi=*/false);

    if (output_file) {
      perftest::ResultsSet results;
      auto* test_case =
          results.AddTestCase("fuchsia.zircon", "BlockDeviceThroughput", "bytes/second");
      double time_in_seconds = static_cast<double>(xfer_duration) / 1e9;
      test_case->AppendValue(static_cast<double>(total_xfer_bytes) / time_in_seconds);
      if (!results.WriteJSONFile(output_file)) {
        return 1;
      }
    }
  } while (loop_forever || (--opt_num_iter > 0));

  return 0;
}
