// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <fidl/fuchsia.hardware.skipblock/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <lib/fzl/owned-vmo-mapper.h>
#include <lib/zx/result.h>
#include <limits.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/process.h>
#include <zircon/rights.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <memory>

#include <fbl/unique_fd.h>
#include <safemath/checked_math.h>

#include "src/storage/lib/block_client/cpp/client.h"
#include "src/storage/lib/buffer/vmoid_registry.h"

namespace {

int Usage() {
  fprintf(stderr, "Usage: dd [OPTIONS]\n");
  fprintf(stderr, "dd can be used to convert and copy files\n");
  fprintf(stderr, " bs=BYTES  : read and write BYTES bytes at a time\n");
  fprintf(stderr, " count=N   : copy only N input blocks\n");
  fprintf(stderr, " ibs=BYTES : read BYTES bytes at a time (default: 512)\n");
  fprintf(stderr, " if=FILE   : read from FILE instead of stdin\n");
  fprintf(stderr, " obs=BYTES : write BYTES bytes at a time (default: 512)\n");
  fprintf(stderr, " of=FILE   : write to FILE instead of stdout\n");
  fprintf(stderr, " seek=N    : skip N obs-sized blocks at start of output\n");
  fprintf(stderr, " skip=N    : skip N ibs-sized blocks at start of input\n");
  fprintf(stderr, " conv=sync : pad input to input block size\n");
  fprintf(stderr,
          " N and BYTES may be followed by the following multiplicitive\n"
          " suffixes: c = 1, w = 2, b = 512, kB = 1000, K = 1024,\n"
          "           MB = 1000 * 1000, M = 1024 * 1024, xM = M,\n"
          "           GB = 1000 * 1000 * 1000, G = 1024 * 1024 * 1024\n");
  fprintf(stderr, " --help : Show this help message\n");
  return EXIT_FAILURE;
}

// Returns "true" if the argument matches the prefix.
// In this case, moves the argument past the prefix.
bool PrefixMatch(const char** arg, std::string_view prefix) {
  if (std::string_view(*arg).starts_with(prefix)) {
    *arg += prefix.length();
    return true;
  }
  return false;
}

#define MAYBE_MULTIPLY_SUFFIX(str, out, suffix, value) \
  if (std::string_view(str) == (suffix)) {             \
    (out) *= (value);                                  \
    return 0;                                          \
  }

// Parse the formatted size string from |s|, and place
// the result in |out|.
//
// Returns 0 on success.
int ParseSize(const char* s, size_t* out) {
  char* endptr;
  if (*s < '0' || *s > '9') {
    goto done;
  }
  *out = strtol(s, &endptr, 10);
  if (*endptr == '\0') {
    return 0;
  }

  MAYBE_MULTIPLY_SUFFIX(endptr, *out, "G", UINT64_C(1) << 30);
  MAYBE_MULTIPLY_SUFFIX(endptr, *out, "GB", UINT64_C(1000) * UINT64_C(1000) * UINT64_C(1000));
  MAYBE_MULTIPLY_SUFFIX(endptr, *out, "xM", UINT64_C(1) << 20);
  MAYBE_MULTIPLY_SUFFIX(endptr, *out, "M", UINT64_C(1) << 20);
  MAYBE_MULTIPLY_SUFFIX(endptr, *out, "MB", UINT64_C(1000) * UINT64_C(1000));
  MAYBE_MULTIPLY_SUFFIX(endptr, *out, "K", UINT64_C(1) << 10);
  MAYBE_MULTIPLY_SUFFIX(endptr, *out, "kB", UINT64_C(1000));
  MAYBE_MULTIPLY_SUFFIX(endptr, *out, "b", UINT64_C(512));
  MAYBE_MULTIPLY_SUFFIX(endptr, *out, "w", UINT64_C(2));
  MAYBE_MULTIPLY_SUFFIX(endptr, *out, "c", UINT64_C(1));

done:
  fprintf(stderr, "Couldn't parse size string: %s\n", s);
  return -1;
}

struct DdOptions {
  bool use_count;
  size_t count;
  size_t input_bs;
  size_t input_skip;
  size_t output_bs;
  size_t output_seek;
  char input[PATH_MAX];
  char output[PATH_MAX];
};

int ParseArgs(int argc, const char** argv, DdOptions* options) {
  while (argc > 1) {
    const char* arg = argv[1];
    if (PrefixMatch(&arg, "bs=")) {
      size_t size;
      if (ParseSize(arg, &size)) {
        return Usage();
      }
      options->input_bs = size;
      options->output_bs = size;
    } else if (PrefixMatch(&arg, "count=")) {
      if (ParseSize(arg, &options->count)) {
        return Usage();
      }
      options->use_count = true;
    } else if (PrefixMatch(&arg, "ibs=")) {
      if (ParseSize(arg, &options->input_bs)) {
        return Usage();
      }
    } else if (PrefixMatch(&arg, "obs=")) {
      if (ParseSize(arg, &options->output_bs)) {
        return Usage();
      }
    } else if (PrefixMatch(&arg, "seek=")) {
      if (ParseSize(arg, &options->output_seek)) {
        return Usage();
      }
    } else if (PrefixMatch(&arg, "skip=")) {
      if (ParseSize(arg, &options->input_skip)) {
        return Usage();
      }
    } else if (PrefixMatch(&arg, "if=")) {
      strncpy(options->input, arg, PATH_MAX);
      options->input[PATH_MAX - 1] = '\0';
    } else if (PrefixMatch(&arg, "of=")) {
      strncpy(options->output, arg, PATH_MAX);
      options->output[PATH_MAX - 1] = '\0';
    } else if (strcmp(arg, "conv=sync") == 0) {
      fprintf(stderr, "TODO: conv=sync and partial reads aren't supported yet\n");
      return Usage();
    } else {
      return Usage();
    }
    argc--;
    argv++;
  }
  return 0;
}

#define MIN(x, y) ((x) < (y) ? (x) : (y))
#define MAX(x, y) ((x) < (y) ? (y) : (x))

bool IsBlockPath(std::string_view path) {
  // E.g.
  //   /dev/class/block/000
  //   /dev/sys/platform/00:2d/ramctl/ramdisk-0/block
  //   /block/000/fuchsia.storage.block.Block
  return (path.starts_with("/dev") && path.ends_with("/block")) ||
         path.starts_with("/dev/class/block") ||
         (path.starts_with("/block") && path.ends_with("fuchsia.storage.block.Block"));
}

bool IsSkipBlockPath(std::string_view path) {
  // E.g. /dev/class/skip-block/000 or
  // /dev/sys/platform/raw_nand/aml-raw_nand/nand/zircon-a/skip-block
  return (path.starts_with("/dev") && path.ends_with("/skip-block")) ||
         path.starts_with("/dev/class/skip-block");
}

zx::result<fidl::ClientEnd<fuchsia_hardware_skipblock::SkipBlock>> GetSkipBlockClient(
    const char* path, size_t* block_size) {
  zx::result client = component::Connect<fuchsia_hardware_skipblock::SkipBlock>(path);
  if (client.is_error()) {
    return client.take_error();
  }
  fidl::WireResult res = fidl::WireCall(*client)->GetPartitionInfo();
  if (!res.ok()) {
    return zx::error(res.error().status());
  }
  if (res.value().status != ZX_OK) {
    return zx::error(res.value().status);
  }
  *block_size = res.value().partition_info.block_size_bytes;
  return zx::ok(std::move(*client));
}

zx::result<std::unique_ptr<block_client::Client>> GetBlockClient(const char* path,
                                                                 size_t* block_size) {
  zx::result client = component::Connect<fuchsia_storage_block::Block>(path);
  if (client.is_error()) {
    return client.take_error();
  }
  fidl::WireResult info = fidl::WireCall(*client)->GetInfo();
  if (!info.ok()) {
    return zx::error(info.status());
  }
  auto [session, server] = fidl::Endpoints<fuchsia_storage_block::Session>::Create();
  if (fidl::OneWayStatus result = fidl::WireCall(*client)->OpenSession(std::move(server));
      !result.ok()) {
    return zx::error(result.status());
  }
  fidl::WireResult fifo_result = fidl::WireCall(session)->GetFifo();
  if (!fifo_result.ok()) {
    return zx::error(fifo_result.status());
  }
  fit::result fifo_response = fifo_result.value();
  if (fifo_response.is_error()) {
    return fifo_response.take_error();
  }
  *block_size = info.value()->info.block_size;
  return zx::ok(std::make_unique<block_client::Client>(std::move(session),
                                                       std::move(fifo_response.value()->fifo)));
}

class Target {
 public:
  ~Target() {
    if (vmoid_.IsAttached()) {
      ZX_DEBUG_ASSERT(variant_ == Variant::kBlock && block_);
      BlockFifoRequest request;
      request.vmoid = vmoid_.TakeId();
      request.command = {.opcode = BLOCK_OPCODE_CLOSE_VMO, .flags = 0};
      if (zx_status_t status = block_->Transaction(&request, 1); status != ZX_OK) {
        fprintf(stderr, "Failed to detach VMO: %s\n", zx_status_get_string(status));
      }
    }
  }
  Target(Target&& o) = default;
  Target& operator=(Target&& o) = default;
  Target(const Target&) = delete;
  Target& operator=(const Target&) = delete;

  static zx::result<Target> Open(const char* path, size_t target_block_size) {
    size_t block_size;
    if (IsSkipBlockPath(path)) {
      zx::result client = GetSkipBlockClient(path, &block_size);
      if (client.is_error()) {
        return client.take_error();
      }
      Target target;
      target.variant_ = Variant::kSkipBlock;
      target.skip_block_ = std::move(*client);
      if (target_block_size % block_size) {
        fprintf(stderr, "Block size (%zu) must be a multiple of device's blksize %zu\n",
                target_block_size, block_size);
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      target.block_size_ = block_size;
      return zx::ok(std::move(target));
    }
    if (IsBlockPath(path)) {
      zx::result client = GetBlockClient(path, &block_size);
      if (client.is_error()) {
        return client.take_error();
      }
      Target target;
      target.variant_ = Variant::kBlock;
      target.block_ = std::move(*client);
      if (target_block_size % block_size) {
        fprintf(stderr, "Block size (%zu) must be a multiple of device's blksize %zu\n",
                target_block_size, block_size);
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      target.block_size_ = block_size;
      return zx::ok(std::move(target));
    }
    // Use regular file I/O for the rest.
    fbl::unique_fd fd(open(path, O_RDWR | O_CREAT));
    if (!fd) {
      return zx::error(ZX_ERR_BAD_PATH);
    }
    Target target;
    target.variant_ = Variant::kFile;
    target.fd_ = std::move(fd);
    target.block_size_ = target_block_size;
    return zx::ok(std::move(target));
  }

  zx::result<> SetBuffer(const fzl::OwnedVmoMapper& vmo) {
    buf_size_ = vmo.size();
    switch (variant_) {
      case Variant::kFile: {
        buf_ = vmo.start();
        return zx::ok();
      }
      case Variant::kBlock: {
        zx::result vmoid = block_->RegisterVmo(vmo.vmo());
        if (vmoid.is_error()) {
          return vmoid.take_error();
        }
        vmoid_ = std::move(*vmoid);
        return zx::ok();
      }
      case Variant::kSkipBlock: {
        vmo_ = vmo.vmo().borrow();
        return zx::ok();
      }
      case Variant::kUnknown:
        return zx::error(ZX_ERR_BAD_STATE);
    }
  }

  zx::result<> ReadOne() {
    auto nblocks = static_cast<uint32_t>(buf_size_ / block_size_);
    switch (variant_) {
      case Variant::kFile: {
        ssize_t res = read(fd_.get(), buf_, buf_size_);
        if (res < static_cast<ssize_t>(buf_size_)) {
          if (res == 0) {
            return zx::error(ZX_ERR_STOP);
          }
          return zx::error(ZX_ERR_IO);
        }
        break;
      }
      case Variant::kBlock: {
        BlockFifoRequest request;
        request.vmoid = vmoid_.get();
        request.command = {.opcode = BLOCK_OPCODE_READ, .flags = 0};
        request.length = nblocks;
        request.vmo_offset = 0;
        request.dev_offset = cur_block_;
        if (zx_status_t status = block_->Transaction(&request, 1); status != ZX_OK) {
          return zx::error(status);
        }
        break;
      }
      case Variant::kSkipBlock: {
        zx::vmo vmo;
        if (zx_status_t status = vmo_->duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo); status != ZX_OK) {
          return zx::error(status);
        }
        fuchsia_hardware_skipblock::wire::ReadWriteOperation op{
            .vmo = std::move(vmo),
            .vmo_offset = 0,
            .block = static_cast<uint32_t>(cur_block_),
            .block_count = static_cast<uint32_t>(nblocks),
        };
        fidl::WireResult res = fidl::WireCall(skip_block_)->Read(std::move(op));
        if (!res.ok()) {
          return zx::error(res.status());
        }
        if (res.value().status != ZX_OK) {
          return zx::error(res.value().status);
        }
        break;
      }
      case Variant::kUnknown: {
        return zx::error(ZX_ERR_BAD_STATE);
      }
    }
    cur_block_ += nblocks;
    return zx::ok();
  }

  zx::result<> WriteOne() {
    auto nblocks = static_cast<uint32_t>(buf_size_ / block_size_);
    switch (variant_) {
      case Variant::kFile: {
        ssize_t res = write(fd_.get(), buf_, buf_size_);
        if (res < static_cast<ssize_t>(buf_size_)) {
          return zx::error(ZX_ERR_IO);
        }
        break;
      }
      case Variant::kBlock: {
        BlockFifoRequest request;
        request.vmoid = vmoid_.get();
        request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};
        request.length = nblocks;
        request.vmo_offset = 0;
        request.dev_offset = cur_block_;
        if (zx_status_t status = block_->Transaction(&request, 1); status != ZX_OK) {
          return zx::error(status);
        }
        break;
      }
      case Variant::kSkipBlock: {
        zx::vmo vmo;
        if (zx_status_t status = vmo_->duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo); status != ZX_OK) {
          return zx::error(status);
        }
        fuchsia_hardware_skipblock::wire::ReadWriteOperation op{
            .vmo = std::move(vmo),
            .vmo_offset = 0,
            .block = static_cast<uint32_t>(cur_block_),
            .block_count = static_cast<uint32_t>(nblocks),
        };
        fidl::WireResult res = fidl::WireCall(skip_block_)->Write(std::move(op));
        if (!res.ok()) {
          fprintf(stderr, "Failed to write (FIDL): %s\n", res.status_string());
          return zx::error(res.status());
        }
        if (res.value().status != ZX_OK) {
          fprintf(stderr, "Failed to write: %s\n", zx_status_get_string(res.value().status));
          return zx::error(res.value().status);
        }
        break;
      }
      case Variant::kUnknown: {
        return zx::error(ZX_ERR_BAD_STATE);
      }
    }
    cur_block_ += nblocks;
    return zx::ok();
  }

  zx::result<> Seek(uint64_t block) {
    cur_block_ = block;
    if (variant_ == Variant::kFile) {
      off_t seek = safemath::CheckMul(safemath::checked_cast<off_t>(cur_block_), block_size_)
                       .Cast<off_t>()
                       .ValueOrDie();
      if (lseek(fd_.get(), seek, SEEK_SET) != seek) {
        return zx::error(ZX_ERR_IO);
      }
    }
    return zx::ok();
  }

 private:
  Target() = default;
  enum class Variant {
    kFile,
    kBlock,
    kSkipBlock,
    kUnknown,
  } variant_ = Variant::kUnknown;

  uint64_t buf_size_ = 0;
  uint64_t block_size_ = 0;
  uint64_t cur_block_ = 0;

  // kFile fields
  fbl::unique_fd fd_;
  void* buf_ = nullptr;
  // kBlock fields
  std::unique_ptr<block_client::Client> block_;
  storage::OwnedVmoid vmoid_;
  // kSkipBlock Fields
  fidl::ClientEnd<fuchsia_hardware_skipblock::SkipBlock> skip_block_;
  zx::unowned_vmo vmo_;
};

}  // namespace

int main(int argc, const char** argv) {
  DdOptions options = {};
  options.input_bs = 512;
  options.output_bs = 512;
  if (int r = ParseArgs(argc, argv, &options); r) {
    return r;
  }

  if (options.input_bs == 0 || options.output_bs == 0) {
    fprintf(stderr, "block sizes must be greater than zero\n");
    return EXIT_FAILURE;
  }

  if (MAX(options.input_bs, options.output_bs) % MIN(options.input_bs, options.output_bs) != 0) {
    // We could implement this case if we cared to do so.
    fprintf(stderr, "Input and output block sizes must be multiples\n");
    return EXIT_FAILURE;
  }

  size_t buf_size = MAX(options.output_bs, options.input_bs);

  // Buffer to contain partially read data.  Both `in` and `out` will share this VMO.
  fzl::OwnedVmoMapper vmo;

  // Number of full records copied to/from target
  size_t records_in = 0;
  size_t records_out = 0;

  uint64_t sum_bytes_out = 0;

  zx_time_t start = 0, stop = 0;

  if (*options.input == '\0') {
    fprintf(stderr, "Reading from stdin isn't supported.\n");
    return Usage();
  }
  zx::result target = Target::Open(options.input, options.input_bs);
  if (target.is_error()) {
    fprintf(stderr, "Failed to open %s: %s\n", options.input, target.status_string());
    return EXIT_FAILURE;
  }
  Target in = std::move(*target);

  if (*options.output == '\0') {
    fprintf(stderr, "Writing to stdout isn't supported.\n");
    return Usage();
  }
  target = Target::Open(options.output, options.output_bs);
  if (target.is_error()) {
    fprintf(stderr, "Failed to open %s: %s\n", options.output, target.status_string());
    return EXIT_FAILURE;
  }
  Target out = std::move(*target);

  if (zx_status_t status = vmo.CreateAndMap(buf_size, "buf"); status != ZX_OK) {
    fprintf(stderr, "Failed to create VMO: %s\n", zx_status_get_string(status));
    return EXIT_FAILURE;
  }
  if (zx::result status = in.SetBuffer(vmo); status.is_error()) {
    fprintf(stderr, "Failed to set buffer: %s\n", status.status_string());
    return EXIT_FAILURE;
  }
  if (zx::result status = out.SetBuffer(vmo); status.is_error()) {
    fprintf(stderr, "Failed to set buffer: %s\n", status.status_string());
    return EXIT_FAILURE;
  }
  if (options.input_skip) {
    if (zx::result status = in.Seek(options.input_skip); status.is_error()) {
      fprintf(stderr, "Failed to skip: %s\n", status.status_string());
      return EXIT_FAILURE;
    }
  }
  if (options.output_seek) {
    if (zx::result status = out.Seek(options.output_seek); status.is_error()) {
      fprintf(stderr, "Failed to seek: %s\n", status.status_string());
      return EXIT_FAILURE;
    }
  }

  start = zx_clock_get_monotonic();
  int res = EXIT_FAILURE;
  while (true) {
    if (options.use_count && !options.count) {
      res = EXIT_SUCCESS;
      break;
    }
    --options.count;

    if (zx::result status = in.ReadOne(); status.is_error()) {
      if (status.status_value() != ZX_ERR_STOP) {
        fprintf(stderr, "Failed to read: %s\n", status.status_string());
      }
      break;
    }
    records_in++;

    if (zx::result status = out.WriteOne(); status.is_error()) {
      fprintf(stderr, "Failed to write: %s\n", status.status_string());
      break;
    }
    records_out++;
  }

  stop = zx_clock_get_monotonic();
  printf("%zu+0 records in\n", records_in);
  printf("%zu+0 records out\n", records_out);
  sum_bytes_out = records_out * options.output_bs;
  if ((start != 0) && (stop > start)) {
    fprintf(stderr, "%zu bytes copied, %zu bytes/s\n", sum_bytes_out,
            sum_bytes_out * ZX_SEC(1) / (stop - start));
  } else {
    printf("%zu bytes copied\n", sum_bytes_out);
  }

  return res;
}
