// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/memory/thrasher/lib.h"

#include <dirent.h>
#include <fidl/fuchsia.fxfs/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <fidl/fuchsia.pkg/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/io.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/process.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <algorithm>
#include <atomic>
#include <bit>
#include <chrono>
#include <cstdint>
#include <functional>
#include <iomanip>
#include <iostream>
#include <memory>
#include <optional>
#include <random>
#include <sstream>
#include <string>
#include <thread>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

#include "src/storage/lib/vfs/cpp/vfs_types.h"

namespace fio = fuchsia_io;
namespace ffxfs = fuchsia_fxfs;

namespace {

constexpr size_t kBitsPerUint64 = 64;

class PageTracker {
 public:
  explicit PageTracker(size_t num_pages) : num_bits_(num_pages) {
    num_words_ = (num_pages + kBitsPerUint64 - 1) / kBitsPerUint64;
    bits_ = std::make_unique<std::atomic<uint64_t>[]>(num_words_);
    for (size_t i = 0; i < num_words_; ++i) {
      bits_[i].store(0, std::memory_order_relaxed);
    }
  }

  // Marks a page as having been touched. This is thread-safe.
  void MarkPage(size_t page_index) {
    if (page_index >= num_bits_) {
      return;
    }
    size_t word_index = page_index / kBitsPerUint64;
    uint64_t bit_mask = 1ULL << (page_index % kBitsPerUint64);
    bits_[word_index].fetch_or(bit_mask, std::memory_order_relaxed);
  }

  // Counts the number of unique pages marked since the last call and resets the tracker. This is
  // thread-safe.
  uint64_t CountAndReset() {
    uint64_t count = 0;
    for (size_t i = 0; i < num_words_; ++i) {
      uint64_t value = bits_[i].exchange(0, std::memory_order_relaxed);
      count += std::popcount(value);
    }
    return count;
  }

 private:
  size_t num_bits_;
  size_t num_words_;
  std::unique_ptr<std::atomic<uint64_t>[]> bits_;
};

struct MappedBuffer {
  void* mapped = nullptr;
  size_t size = 0;
  zx::vmo vmo;

  MappedBuffer() = default;
  MappedBuffer(void* m, size_t s, zx::vmo v) : mapped(m), size(s), vmo(std::move(v)) {}
  ~MappedBuffer() {
    if (mapped) {
      zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(mapped), size);
    }
  }
  MappedBuffer(const MappedBuffer&) = delete;
  MappedBuffer& operator=(const MappedBuffer&) = delete;
  MappedBuffer(MappedBuffer&& other) noexcept
      : mapped(other.mapped), size(other.size), vmo(std::move(other.vmo)) {
    other.mapped = nullptr;
    other.size = 0;
  }
  MappedBuffer& operator=(MappedBuffer&& other) noexcept {
    if (this != &other) {
      if (mapped) {
        zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(mapped), size);
      }
      mapped = other.mapped;
      size = other.size;
      vmo = std::move(other.vmo);
      other.mapped = nullptr;
      other.size = 0;
    }
    return *this;
  }
};

struct MappedFile {
  void* ptr = nullptr;
  size_t size = 0;
  zx::vmo vmo;
  std::string filename;

  MappedFile() = default;
  MappedFile(void* p, size_t s, zx::vmo v, std::string f)
      : ptr(p), size(s), vmo(std::move(v)), filename(std::move(f)) {}
  ~MappedFile() {
    if (ptr) {
      zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(ptr), size);
    }
  }
  MappedFile(const MappedFile&) = delete;
  MappedFile& operator=(const MappedFile&) = delete;
  MappedFile(MappedFile&& other) noexcept
      : ptr(other.ptr),
        size(other.size),
        vmo(std::move(other.vmo)),
        filename(std::move(other.filename)) {
    other.ptr = nullptr;
    other.size = 0;
  }
  MappedFile& operator=(MappedFile&& other) noexcept {
    if (this != &other) {
      if (ptr) {
        zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(ptr), size);
      }
      ptr = other.ptr;
      size = other.size;
      vmo = std::move(other.vmo);
      filename = std::move(other.filename);
      other.ptr = nullptr;
      other.size = 0;
    }
    return *this;
  }
};

struct MappedBlob {
  void* ptr = nullptr;
  size_t size = 0;
  zx::vmo vmo;
  std::string merkle_root;

  MappedBlob() = default;
  MappedBlob(void* p, size_t s, zx::vmo v, std::string mr)
      : ptr(p), size(s), vmo(std::move(v)), merkle_root(std::move(mr)) {}
  ~MappedBlob() {
    if (ptr) {
      zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(ptr), size);
    }
  }
  MappedBlob(const MappedBlob&) = delete;
  MappedBlob& operator=(const MappedBlob&) = delete;
  MappedBlob(MappedBlob&& other) noexcept
      : ptr(other.ptr),
        size(other.size),
        vmo(std::move(other.vmo)),
        merkle_root(std::move(other.merkle_root)) {
    other.ptr = nullptr;
    other.size = 0;
  }
  MappedBlob& operator=(MappedBlob&& other) noexcept {
    if (this != &other) {
      if (ptr) {
        zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(ptr), size);
      }
      ptr = other.ptr;
      size = other.size;
      vmo = std::move(other.vmo);
      merkle_root = std::move(other.merkle_root);
      other.ptr = nullptr;
      other.size = 0;
    }
    return *this;
  }
};

struct AnonThrashState {
  ThrashConfig config;
  std::optional<MappedBuffer> buffer;
  std::unique_ptr<PageTracker> page_tracker;
  std::atomic<uint64_t> pages_touched_counter{0};
  std::atomic<bool> keep_running{true};
  std::thread status_thread;
  std::vector<std::thread> worker_threads;
};

struct MmapThrashState {
  ThrashConfig config;
  std::optional<MappedFile> mapped_file;
  std::unique_ptr<PageTracker> page_tracker;
  std::atomic<uint64_t> pages_touched_counter{0};
  std::atomic<bool> keep_running{true};
  std::thread status_thread;
  std::vector<std::thread> worker_threads;
};

struct DirThrashState {
  ThrashConfig config;
  std::vector<MappedFile> mapped_files;
  std::vector<const uint8_t*> all_pages;
  size_t total_memory_bytes = 0;
  std::unique_ptr<PageTracker> page_tracker;
  std::atomic<uint64_t> pages_touched_counter{0};
  std::atomic<bool> keep_running{true};
  std::thread status_thread;
  std::vector<std::thread> worker_threads;
};

}  // namespace

struct BlobResult {
  std::string merkle_root;
  uint64_t size_bytes;
};

// Helper to convert byte array to hex string
std::string to_hex_string(const std::vector<uint8_t>& bytes) {
  std::ostringstream oss;
  oss << std::hex << std::setfill('0');
  for (uint8_t byte : bytes) {
    oss << std::setw(2) << static_cast<int>(byte);
  }
  return oss.str();
}

void print_vmo_info(const zx::vmo& vmo, const char* name) {
  if (!vmo.is_valid()) {
    return;
  }

  zx_info_handle_basic_t basic_info;
  zx_status_t status =
      vmo.get_info(ZX_INFO_HANDLE_BASIC, &basic_info, sizeof(basic_info), nullptr, nullptr);
  if (status != ZX_OK) {
    return;
  }

  char vmo_name[ZX_MAX_NAME_LEN] = {};
  vmo.get_property(ZX_PROP_NAME, vmo_name, sizeof(vmo_name));

  zx_info_vmo_t vmo_info;
  status = vmo.get_info(ZX_INFO_VMO, &vmo_info, sizeof(vmo_info), nullptr, nullptr);
  if (status != ZX_OK) {
    return;
  }

  double size_val = static_cast<double>(vmo_info.size_bytes);
  std::string units = "B";
  if (size_val >= 1024.0) {
    size_val /= 1024.0;
    units = "KiB";
  }
  if (size_val >= 1024.0) {
    size_val /= 1024.0;
    units = "MiB";
  }
  if (size_val >= 1024.0) {
    size_val /= 1024.0;
    units = "GiB";
  }

  std::cout << name << " VMO: " << vmo_name << " (koid: " << basic_info.koid << ") "
            << "Size: " << std::fixed << std::setprecision(2) << size_val << units << " ";

  if (std::string(name) == "Blob") {
    std::cout << "Committed: Unknown Populated: Unknown" << std::endl;
  } else {
    std::cout << "Committed: " << std::fixed << std::setprecision(2)
              << static_cast<double>(vmo_info.committed_bytes) / (1024.0 * 1024.0) << " MiB "
              << "Populated: " << std::fixed << std::setprecision(2)
              << static_cast<double>(vmo_info.populated_bytes) / (1024.0 * 1024.0) << " MiB"
              << std::endl;
  }
}

namespace {

std::optional<MappedFile> map_file(const std::string& filename) {
  zx::channel client, server;
  zx_status_t status = zx::channel::create(0, &client, &server);
  if (status != ZX_OK) {
    return std::nullopt;
  }
  status = fdio_open3(filename.c_str(),
                      static_cast<uint64_t>(fuchsia::io::Flags::PERM_READ_BYTES |
                                            fuchsia::io::Flags::PERM_GET_ATTRIBUTES),
                      server.release());
  if (status != ZX_OK) {
    return std::nullopt;
  }
  fidl::WireSyncClient<fuchsia_io::File> file(fidl::ClientEnd<fuchsia_io::File>(std::move(client)));

  auto result = file->GetAttributes(fuchsia_io::wire::NodeAttributesQuery::kContentSize);
  if (!result.ok() || result.value().is_error()) {
    return std::nullopt;
  }
  uint64_t size = result.value()->immutable_attributes.content_size();

  auto memory_result = file->GetBackingMemory(fuchsia_io::wire::VmoFlags::kRead);
  if (!memory_result.ok() || memory_result.value().is_error()) {
    return std::nullopt;
  }
  zx::vmo vmo = std::move(memory_result.value().value()->vmo);
  zx_info_handle_basic_t info;
  status = vmo.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) {
    return std::nullopt;
  }

  uintptr_t mapped_addr;
  status = zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, vmo, 0, size, &mapped_addr);
  if (status != ZX_OK) {
    return std::nullopt;
  }
  void* mapped = reinterpret_cast<void*>(mapped_addr);

  // Touch each page to ensure it's faulted in.
  volatile uint8_t value;
  const size_t page_size = zx_system_get_page_size();
  for (size_t i = 0; i < size; i += page_size) {
    value = static_cast<const uint8_t*>(mapped)[i];
  }
  (void)value;

  size_t mapped_size_bytes = (size + page_size - 1) & -page_size;
  double mapped_size_val = static_cast<double>(mapped_size_bytes);
  std::string units = " MiB";
  double display_size = mapped_size_val / (1024.0 * 1024.0);
  if (display_size < 1.0) {
    display_size = mapped_size_val / 1024.0;
    units = " KiB";
  }
  return MappedFile{mapped, (size_t)size, std::move(vmo), filename};
}

std::optional<MappedBuffer> allocate_and_touch_buffer(size_t buffer_size_bytes) {
  zx::vmo vmo;
  zx_status_t status = zx::vmo::create(buffer_size_bytes, 0, &vmo);
  if (status != ZX_OK) {
    return std::nullopt;
  }

  const char name[] = "thrashed_memory";
  vmo.set_property(ZX_PROP_NAME, name, sizeof(name) - 1);

  uintptr_t mapped_addr;
  status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                      buffer_size_bytes, &mapped_addr);
  if (status != ZX_OK) {
    return std::nullopt;
  }
  uint8_t* buffer = reinterpret_cast<uint8_t*>(mapped_addr);

  // Fill the buffer with a pseudo-random, but deterministic, pattern that will have
  // a reasonable compression ratio.
  uint32_t seed = 0;
  for (size_t i = 0; i < buffer_size_bytes; ++i) {
    // Use a simple linear congruential generator (LCG).
    seed = (seed * 1103515245 + 12345) & 0xFFFFFFFF;
    buffer[i] = static_cast<uint8_t>((seed >> 16) & 0xFF);
  }

  return MappedBuffer{buffer, buffer_size_bytes, std::move(vmo)};
}

void thrash_memory_worker(uint8_t* buffer, size_t size, int bursts_per_second,
                          std::atomic<uint64_t>& pages_touched_counter,
                          std::atomic<bool>& keep_running, int pages_per_read,
                          int consecutive_pages_per_read, bool write, PageTracker* page_tracker) {
  const size_t page_size = zx_system_get_page_size();
  const size_t num_pages = size / page_size;

  const auto delay_between_bursts =
      std::chrono::microseconds(static_cast<long long>(1'000'000.0 / bursts_per_second));

  // Volatile to prevent the compiler from optimizing away the read/write.
  volatile uint8_t value;

  std::random_device rd;
  std::mt19937 gen(rd());
  std::uniform_int_distribution<size_t> page_distrib(0, num_pages - 1);
  std::uniform_int_distribution<size_t> pages_to_read_distrib(1, pages_per_read);
  std::uniform_int_distribution<size_t> consecutive_pages_to_read_distrib(
      1, consecutive_pages_per_read);

  while (keep_running.load(std::memory_order_relaxed)) {
    size_t pages_to_read = pages_to_read_distrib(gen);
    for (size_t i = 0; i < pages_to_read; ++i) {
      size_t start_page_index = page_distrib(gen);
      size_t consecutive_pages_to_read = consecutive_pages_to_read_distrib(gen);
      for (size_t j = 0; j < consecutive_pages_to_read; ++j) {
        size_t current_page_index = start_page_index + j;
        if (current_page_index < num_pages) {
          uint8_t* byte_to_modify = &buffer[current_page_index * page_size];
          value = *byte_to_modify;
          if (write) {
            *byte_to_modify = value + 1;
          }
          pages_touched_counter.fetch_add(1, std::memory_order_relaxed);
          if (page_tracker) {
            page_tracker->MarkPage(current_page_index);
          }
        }
      }
    }
    std::this_thread::sleep_for(delay_between_bursts);
  }
}

class AnonThrasher : public Thrasher, public std::enable_shared_from_this<AnonThrasher> {
 public:
  AnonThrasher(ThrashConfig config, size_t buffer_size_bytes)
      : buffer_size_bytes_(buffer_size_bytes), config_(std::move(config)) {}

 private:
  size_t buffer_size_bytes_;
  ThrashConfig config_;
  std::optional<MappedBuffer> buffer_;
  std::unique_ptr<PageTracker> page_tracker_;

  void Initialize(fit::function<void(zx_status_t)> on_initialized) override {
    if (buffer_size_bytes_ == 0) {
      on_initialized(ZX_ERR_INVALID_ARGS);
      return;
    }
    buffer_ = allocate_and_touch_buffer(buffer_size_bytes_);
    if (!buffer_) {
      on_initialized(ZX_ERR_NO_MEMORY);
      return;
    }
    const size_t total_pages = buffer_size_bytes_ / zx_system_get_page_size();
    page_tracker_ = std::make_unique<PageTracker>(total_pages);
    on_initialized(ZX_OK);
  }

  void Start(std::shared_ptr<ThrashCallback> callback,
             std::shared_ptr<StatusCallback> status_callback) override {
    callback_ = callback;
    status_callback_ = status_callback;
    if (status_callback_) {
      status_thread_ = std::thread([self = shared_from_this()]() {
        uint64_t last_touches = 0;
        auto start_time = zx::clock::get_monotonic();
        auto last_time = start_time;

        while (self->keep_running_.load(std::memory_order_relaxed)) {
          std::this_thread::sleep_for(std::chrono::milliseconds(self->config_.status_interval_ms));
          if (!self->keep_running_.load(std::memory_order_relaxed))
            break;

          uint64_t current_touches = self->pages_touched_counter_.load(std::memory_order_relaxed);
          uint64_t delta = current_touches - last_touches;
          last_touches = current_touches;

          uint64_t distinct_delta = self->page_tracker_->CountAndReset();

          auto current_time = zx::clock::get_monotonic();
          auto time_delta = current_time - last_time;
          auto total_time = current_time - start_time;
          last_time = current_time;

          ThrashStatus status = {
              .thrasher_type = "anon",
              .total_memory_bytes = self->buffer_->size,
              .touches_delta = delta,
              .total_touches = current_touches,
              .distinct_pages_delta = distinct_delta,
              .time_delta = time_delta,
              .total_time = total_time,
          };
          async::PostTask(self->config_.dispatcher,
                          [cb = self->status_callback_, status]() { (*cb)(status); });
        }
      });
    }

    for (int i = 0; i < config_.num_threads; ++i) {
      worker_threads_.emplace_back(thrash_memory_worker, static_cast<uint8_t*>(buffer_->mapped),
                                   buffer_size_bytes_, config_.bursts_per_second,
                                   std::ref(pages_touched_counter_), std::ref(keep_running_),
                                   config_.pages_per_read, config_.consecutive_pages_per_read,
                                   /*write=*/true, page_tracker_.get());
    }

    std::thread([self = shared_from_this()]() {
      if (self->config_.teardown_sleep_fn) {
        self->config_.teardown_sleep_fn(zx::sec(self->config_.run_for_seconds));
      } else {
        std::this_thread::sleep_for(std::chrono::seconds(self->config_.run_for_seconds));
      }
      self->keep_running_.store(false, std::memory_order_relaxed);

      for (auto& t : self->worker_threads_) {
        t.join();
      }

      if (self->status_thread_.joinable()) {
        self->status_thread_.join();
      }

      if (self->callback_) {
        std::vector<zx::vmo> vmos;
        vmos.push_back(std::move(self->buffer_->vmo));
        async::PostTask(
            self->config_.dispatcher,
            [cb = self->callback_, vmos = std::move(vmos)]() mutable { (*cb)(std::move(vmos)); });
      }
    }).detach();
  }

  std::atomic<uint64_t> pages_touched_counter_{0};
  std::atomic<bool> keep_running_{true};
  std::thread status_thread_;
  std::vector<std::thread> worker_threads_;
  std::shared_ptr<ThrashCallback> callback_;
  std::shared_ptr<StatusCallback> status_callback_;
};

class MmapThrasher : public Thrasher, public std::enable_shared_from_this<MmapThrasher> {
 public:
  MmapThrasher(ThrashConfig config, std::string filename)
      : filename_(std::move(filename)), config_(std::move(config)) {}

 private:
  std::string filename_;
  ThrashConfig config_;
  std::optional<MappedFile> mapped_file_;
  std::unique_ptr<PageTracker> page_tracker_;
  std::atomic<uint64_t> pages_touched_counter_{0};
  std::atomic<bool> keep_running_{true};
  std::thread status_thread_;
  std::vector<std::thread> worker_threads_;
  std::shared_ptr<ThrashCallback> callback_;
  std::shared_ptr<StatusCallback> status_callback_;

  void Initialize(fit::function<void(zx_status_t)> on_initialized) override {
    auto mapped_file_opt = map_file(filename_);
    if (!mapped_file_opt) {
      std::cerr << "Failed to map file: " << filename_ << std::endl;
      on_initialized(ZX_ERR_IO);
      return;
    }
    mapped_file_ = std::move(mapped_file_opt);
    const size_t total_pages = mapped_file_->size / zx_system_get_page_size();
    page_tracker_ = std::make_unique<PageTracker>(total_pages);
    on_initialized(ZX_OK);
  }

  void Start(std::shared_ptr<ThrashCallback> callback,
             std::shared_ptr<StatusCallback> status_callback) override {
    callback_ = callback;
    status_callback_ = status_callback;
    if (status_callback_) {
      status_thread_ = std::thread([self = shared_from_this()]() {
        uint64_t last_touches = 0;
        auto start_time = zx::clock::get_monotonic();
        auto last_time = start_time;

        while (self->keep_running_.load(std::memory_order_relaxed)) {
          std::this_thread::sleep_for(std::chrono::milliseconds(self->config_.status_interval_ms));
          if (!self->keep_running_.load(std::memory_order_relaxed))
            break;

          uint64_t current_touches = self->pages_touched_counter_.load(std::memory_order_relaxed);
          uint64_t delta = current_touches - last_touches;
          last_touches = current_touches;

          uint64_t distinct_delta = self->page_tracker_->CountAndReset();

          auto current_time = zx::clock::get_monotonic();
          auto time_delta = current_time - last_time;
          auto total_time = current_time - start_time;
          last_time = current_time;

          ThrashStatus status = {
              .thrasher_type = "mmap",
              .total_memory_bytes = self->mapped_file_->size,
              .touches_delta = delta,
              .total_touches = current_touches,
              .distinct_pages_delta = distinct_delta,
              .time_delta = time_delta,
              .total_time = total_time,
          };
          async::PostTask(self->config_.dispatcher,
                          [cb = self->status_callback_, status]() { (*cb)(status); });
        }
      });
    }

    for (int i = 0; i < config_.num_threads; ++i) {
      worker_threads_.emplace_back(
          thrash_memory_worker, static_cast<uint8_t*>(mapped_file_->ptr), mapped_file_->size,
          config_.bursts_per_second, std::ref(pages_touched_counter_), std::ref(keep_running_),
          config_.pages_per_read, config_.consecutive_pages_per_read,
          /*write=*/false, page_tracker_.get());  // MmapThrasher only reads
    }

    std::thread([self = shared_from_this()]() {
      if (self->config_.teardown_sleep_fn) {
        self->config_.teardown_sleep_fn(zx::sec(self->config_.run_for_seconds));
      } else {
        std::this_thread::sleep_for(std::chrono::seconds(self->config_.run_for_seconds));
      }
      self->keep_running_.store(false, std::memory_order_relaxed);

      for (auto& thread : self->worker_threads_) {
        thread.join();
      }

      if (self->status_thread_.joinable()) {
        self->status_thread_.join();
      }

      if (self->callback_) {
        std::vector<zx::vmo> vmos;
        vmos.push_back(std::move(self->mapped_file_->vmo));
        async::PostTask(
            self->config_.dispatcher,
            [cb = self->callback_, vmos = std::move(vmos)]() mutable { (*cb)(std::move(vmos)); });
      }
    }).detach();  // Detach the waiter thread to not block the caller.
  }
};

class DirThrasher : public Thrasher, public std::enable_shared_from_this<DirThrasher> {
 public:
  DirThrasher(ThrashConfig config, std::string dirname)
      : dirname_(std::move(dirname)), config_(std::move(config)) {}

  void Initialize(fit::function<void(zx_status_t)> on_initialized) override {
    std::vector<std::string> files;
    std::vector<std::string> dirs_to_scan;
    dirs_to_scan.push_back(dirname_);

    while (!dirs_to_scan.empty()) {
      std::string current_dir_name = dirs_to_scan.back();
      dirs_to_scan.pop_back();

      DIR* dir = opendir(current_dir_name.c_str());
      if (!dir) {
        std::cerr << "Failed to open directory: " << current_dir_name << std::endl;
        continue;
      }

      struct dirent* entry;
      while ((entry = readdir(dir)) != nullptr) {
        std::string entry_name = entry->d_name;
        if (entry_name == "." || entry_name == "..") {
          continue;
        }

        std::string full_path = current_dir_name + "/" + entry_name;
        struct stat st;
        if (stat(full_path.c_str(), &st) != 0) {
          std::cerr << "Failed to stat: " << full_path << std::endl;
          continue;
        }

        if (S_ISDIR(st.st_mode)) {
          dirs_to_scan.push_back(full_path);
        } else if (S_ISREG(st.st_mode)) {
          files.push_back(full_path);
        }
      }
      closedir(dir);
    }

    const size_t page_size = zx_system_get_page_size();

    for (const auto& file_path : files) {
      if (auto mapped_file_opt = map_file(file_path)) {
        total_memory_bytes_ += mapped_file_opt->size;
        mapped_files_.push_back(std::move(*mapped_file_opt));
        MappedFile& mf = mapped_files_.back();
        for (size_t i = 0; i < mf.size; i += page_size) {
          all_pages_.push_back(static_cast<const uint8_t*>(mf.ptr) + i);
        }
      } else {
        std::cerr << "Failed to map file: " << file_path << std::endl;
      }
    }

    on_initialized(ZX_OK);
  }

  void Start(std::shared_ptr<ThrashCallback> callback,
             std::shared_ptr<StatusCallback> status_callback) override {
    callback_ = callback;
    status_callback_ = status_callback;
    if (all_pages_.empty()) {
      std::cerr << "DirThrasher: No pages to thrash." << std::endl;
      if (callback_) {
        async::PostTask(config_.dispatcher, [cb = callback_]() mutable { (*cb)({}); });
      }
      return;
    }

    page_tracker_ = std::make_unique<PageTracker>(all_pages_.size());

    if (status_callback_) {
      status_thread_ = std::thread([self = shared_from_this()]() {
        uint64_t last_touches = 0;
        auto start_time = zx::clock::get_monotonic();
        auto last_time = start_time;

        while (self->keep_running_.load(std::memory_order_relaxed)) {
          std::this_thread::sleep_for(std::chrono::milliseconds(self->config_.status_interval_ms));
          if (!self->keep_running_.load(std::memory_order_relaxed))
            break;

          uint64_t current_touches = self->pages_touched_counter_.load(std::memory_order_relaxed);
          uint64_t delta = current_touches - last_touches;
          last_touches = current_touches;

          uint64_t distinct_delta = self->page_tracker_->CountAndReset();

          auto current_time = zx::clock::get_monotonic();
          auto time_delta = current_time - last_time;
          auto total_time = current_time - start_time;
          last_time = current_time;

          ThrashStatus status = {
              .thrasher_type = "dir",
              .total_memory_bytes = self->total_memory_bytes_,
              .touches_delta = delta,
              .total_touches = current_touches,
              .distinct_pages_delta = distinct_delta,
              .time_delta = time_delta,
              .total_time = total_time,
          };
          async::PostTask(self->config_.dispatcher,
                          [cb = self->status_callback_, status]() { (*cb)(status); });
        }
      });
    }

    auto thrash_fn = [self = shared_from_this()](int /*thread_id*/) {
      std::random_device rd;
      std::mt19937 gen(rd());
      std::uniform_int_distribution<size_t> distrib(0, self->all_pages_.size() - 1);
      std::uniform_int_distribution<size_t> pages_to_read_distrib(1, self->config_.pages_per_read);
      std::uniform_int_distribution<size_t> consecutive_pages_to_read_distrib(
          1, self->config_.consecutive_pages_per_read);

      const auto delay_between_bursts = std::chrono::microseconds(
          static_cast<long long>(1'000'000.0 / self->config_.bursts_per_second));

      volatile uint8_t value;
      while (self->keep_running_.load(std::memory_order_relaxed)) {
        size_t pages_to_read = pages_to_read_distrib(gen);
        for (size_t i = 0; i < pages_to_read; ++i) {
          size_t start_page_index = distrib(gen);
          size_t consecutive_pages_to_read = consecutive_pages_to_read_distrib(gen);
          for (size_t j = 0; j < consecutive_pages_to_read; ++j) {
            size_t current_page_index = start_page_index + j;
            if (current_page_index < self->all_pages_.size()) {
              const uint8_t* page = self->all_pages_[current_page_index];
              value = *page;
              (void)value;
              self->pages_touched_counter_.fetch_add(1, std::memory_order_relaxed);
              self->page_tracker_->MarkPage(current_page_index);
            }
          }
        }
        std::this_thread::sleep_for(delay_between_bursts);
      }
    };

    for (int i = 0; i < config_.num_threads; ++i) {
      worker_threads_.emplace_back(thrash_fn, i);
    }

    std::thread([self = shared_from_this()]() {
      if (self->config_.teardown_sleep_fn) {
        self->config_.teardown_sleep_fn(zx::sec(self->config_.run_for_seconds));
      } else {
        std::this_thread::sleep_for(std::chrono::seconds(self->config_.run_for_seconds));
      }
      self->keep_running_.store(false, std::memory_order_relaxed);

      for (auto& thread : self->worker_threads_) {
        thread.join();
      }

      if (self->status_thread_.joinable()) {
        self->status_thread_.join();
      }

      if (self->callback_) {
        std::vector<zx::vmo> vmos;
        for (auto& mf : self->mapped_files_) {
          vmos.push_back(std::move(mf.vmo));
        }
        async::PostTask(
            self->config_.dispatcher,
            [cb = self->callback_, vmos = std::move(vmos)]() mutable { (*cb)(std::move(vmos)); });
      }
    }).detach();
  }

 private:
  std::string dirname_;
  ThrashConfig config_;
  std::vector<MappedFile> mapped_files_;
  std::vector<const uint8_t*> all_pages_;
  size_t total_memory_bytes_ = 0;
  std::unique_ptr<PageTracker> page_tracker_;
  std::atomic<uint64_t> pages_touched_counter_{0};
  std::atomic<bool> keep_running_{true};
  std::thread status_thread_;
  std::vector<std::thread> worker_threads_;
  std::shared_ptr<ThrashCallback> callback_;
  std::shared_ptr<StatusCallback> status_callback_;
};

}  // namespace

class BlobThrasher : public Thrasher, public std::enable_shared_from_this<BlobThrasher> {
 public:
  BlobThrasher(ThrashConfig config, fidl::ClientEnd<ffxfs::BlobReader> blob_reader_client,
               const std::vector<std::string>& merkle_roots, size_t max_blob_size_bytes)
      : config_(std::move(config)),
        merkle_roots_(merkle_roots),
        blob_reader_client_(std::move(blob_reader_client)),
        max_blob_size_bytes_(max_blob_size_bytes) {}

  ~BlobThrasher() override {}

  void Initialize(fit::function<void(zx_status_t)> on_initialized) override {
    on_initialized_ = std::move(on_initialized);
    if (!blob_reader_client_.is_valid()) {
      std::cerr << "Error: No valid blob_reader_client provided." << std::endl;
      FinishInitialize(ZX_ERR_BAD_HANDLE);
      return;
    }
    blob_reader_.emplace(
        fidl::WireClient<ffxfs::BlobReader>(std::move(blob_reader_client_), config_.dispatcher));

    if (config_.bursts_per_second <= 0) {
      std::cerr << "Bursts per second must be greater than 0." << std::endl;
      FinishInitialize(ZX_ERR_INVALID_ARGS);
      return;
    }
    if (config_.num_threads <= 0) {
      std::cerr << "Number of threads must be greater than 0." << std::endl;
      FinishInitialize(ZX_ERR_INVALID_ARGS);
      return;
    }

    StartGetVmos();
  }

  void Start(std::shared_ptr<ThrashCallback> callback,
             std::shared_ptr<StatusCallback> status_callback) override {
    callback_ = callback;
    status_callback_ = status_callback;
    if (config_.verbose) {
      std::cout << "Starting blob thrashing..." << std::endl;
      std::cout << "Parameters: bursts_per_second=" << config_.bursts_per_second
                << ", run_for_seconds=" << config_.run_for_seconds
                << ", num_threads=" << config_.num_threads
                << ", pages_per_read=" << config_.pages_per_read
                << ", consecutive_pages_per_read=" << config_.consecutive_pages_per_read
                << std::endl;
    }

    if (mapped_blobs_.empty()) {
      std::cerr << "No blobs were successfully mapped for thrashing." << std::endl;
      FinishThrashing();
      return;
    }
    std::vector<const uint8_t*> all_pages;
    const size_t page_size = zx_system_get_page_size();
    size_t total_pages = 0;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      if (mapped_blobs_.empty()) {
        std::cerr << "No blobs were successfully mapped for thrashing." << std::endl;
        FinishThrashing();
        return;
      }
      for (const auto& blob : mapped_blobs_) {
        blob_page_offsets_.push_back(total_pages);
        size_t blob_pages = blob.size / page_size;
        total_pages += blob_pages;
        for (size_t i = 0; i < blob.size; i += page_size) {
          all_pages.push_back(static_cast<const uint8_t*>(blob.ptr) + i);
        }
      }
    }
    page_tracker_ = std::make_unique<PageTracker>(total_pages);

    if (config_.verbose) {
      std::lock_guard<std::mutex> lock(mutex_);
      for (const auto& blob : mapped_blobs_) {
        print_vmo_info(blob.vmo, "Blob");
      }
    }

    if (all_pages.empty()) {
      FinishThrashing();
      return;
    }

    if (status_callback_) {
      keep_running_status_thread_.store(true);
      status_thread_ = std::thread([self = shared_from_this()]() {
        uint64_t last_touches = 0;
        auto start_time = zx::clock::get_monotonic();
        auto last_time = start_time;

        while (self->keep_running_status_thread_.load(std::memory_order_relaxed)) {
          std::this_thread::sleep_for(std::chrono::milliseconds(self->config_.status_interval_ms));
          if (!self->keep_running_status_thread_.load(std::memory_order_relaxed))
            break;

          uint64_t current_touches = self->pages_touched_counter_.load(std::memory_order_relaxed);
          uint64_t delta = current_touches - last_touches;
          last_touches = current_touches;

          uint64_t distinct_delta = self->page_tracker_->CountAndReset();

          auto current_time = zx::clock::get_monotonic();
          auto time_delta = current_time - last_time;
          auto total_time = current_time - start_time;
          last_time = current_time;

          ThrashStatus status = {
              .thrasher_type = "blob",
              .total_memory_bytes = self->total_allocated_bytes_,
              .touches_delta = delta,
              .total_touches = current_touches,
              .distinct_pages_delta = distinct_delta,
              .time_delta = time_delta,
              .total_time = total_time,
          };
          async::PostTask(self->config_.dispatcher,
                          [cb = self->status_callback_, status]() { (*cb)(status); });
        }
      });
    }

    auto thrash_fn = [all_pages, self = shared_from_this()](int thread_id) {
      std::random_device rd;
      std::mt19937 gen(rd());
      std::uniform_int_distribution<size_t> distrib(0, all_pages.size() - 1);
      std::uniform_int_distribution<size_t> pages_to_read_distrib(1, self->config_.pages_per_read);
      std::uniform_int_distribution<size_t> consecutive_pages_to_read_distrib(
          1, self->config_.consecutive_pages_per_read);

      const auto delay_between_bursts = std::chrono::microseconds(
          static_cast<long long>(1'000'000.0 / self->config_.bursts_per_second));

      volatile uint8_t value;
      while (self->keep_running_status_thread_.load(std::memory_order_relaxed)) {
        size_t pages_to_read = pages_to_read_distrib(gen);
        for (size_t i = 0; i < pages_to_read; ++i) {
          size_t start_page_index = distrib(gen);
          size_t consecutive_pages_to_read = consecutive_pages_to_read_distrib(gen);
          for (size_t j = 0; j < consecutive_pages_to_read; ++j) {
            size_t current_page_index = start_page_index + j;
            if (current_page_index < all_pages.size()) {
              const uint8_t* page = all_pages[current_page_index];
              value = *page;
              (void)value;
              self->pages_touched_counter_.fetch_add(1, std::memory_order_relaxed);
              self->page_tracker_->MarkPage(current_page_index);
            }
          }
        }
        std::this_thread::sleep_for(delay_between_bursts);
      }
    };

    for (int i = 0; i < config_.num_threads; ++i) {
      worker_threads_.emplace_back(thrash_fn, i);
    }

    waiter_thread_ = std::thread([self = shared_from_this()]() mutable {
      if (self->config_.teardown_sleep_fn) {
        self->config_.teardown_sleep_fn(zx::sec(self->config_.run_for_seconds));
      } else {
        std::this_thread::sleep_for(std::chrono::seconds(self->config_.run_for_seconds));
      }
      self->keep_running_status_thread_.store(false, std::memory_order_relaxed);

      for (auto& thread : self->worker_threads_) {
        if (thread.joinable()) {
          thread.join();
        }
      }
      if (self->status_thread_.joinable()) {
        self->status_thread_.join();
      }
      async::PostTask(self->config_.dispatcher, [self] { self->FinishThrashing(); });
    });
    waiter_thread_.detach();  // Detach the waiter thread to not block the caller.
  }

 private:
  void StartGetVmos() {
    if (merkle_roots_.empty()) {
      AllGetVmosDone(ZX_OK);
      return;
    }
    IssueGetVmo(0);
  }

  // Issues a FIDL call to BlobReader.GetVmo for the blob at the given index in merkle_roots_.
  // This function is called recursively to fetch VMOs one by one.
  void IssueGetVmo(size_t index) {
    if (index >= merkle_roots_.size()) {
      AllGetVmosDone(ZX_OK);
      return;
    }
    const auto& merkle_root = merkle_roots_[index];
    if (merkle_root.length() != 64) {
      std::cerr << "Invalid merkle root length: " << merkle_root << std::endl;
      IssueGetVmo(index + 1);
      return;
    }

    std::array<uint8_t, 32> merkle_array;
    for (size_t i = 0; i < 32; ++i) {
      std::string byteString = merkle_root.substr(i * 2, 2);
      merkle_array[i] = static_cast<uint8_t>(strtol(byteString.c_str(), nullptr, 16));
    }

    fidl::Array<uint8_t, 32> fidl_merkle_array;
    memcpy(fidl_merkle_array.data(), merkle_array.data(), merkle_array.size());

    {
      std::lock_guard<std::mutex> lock(mutex_);
      if (logged_connection_error_) {
        AllGetVmosDone(ZX_ERR_PEER_CLOSED);
        return;
      }
    }

    (*blob_reader_)
        ->GetVmo(fidl_merkle_array)
        .Then([self = shared_from_this(), merkle_root,
               index](fidl::WireUnownedResult<ffxfs::BlobReader::GetVmo>& result) {
          self->OnGetVmoDone(merkle_root, index, result);
        });
  }

  void OnGetVmoDone(const std::string& merkle_root, size_t index,
                    fidl::WireUnownedResult<ffxfs::BlobReader::GetVmo>& result) {
    if (!result.ok()) {
      bool should_call_done = false;
      {
        std::lock_guard<std::mutex> lock(mutex_);
        if (!logged_connection_error_) {
          std::cerr << "[THRASHER_CLIENT] GetVmo FIDL call failed for " << merkle_root << ": "
                    << result.status_string() << " (Status: " << result.status() << ")"
                    << " - Description: " << result.error().FormatDescription() << std::endl;
          if (result.status() == ZX_ERR_NOT_FOUND || result.status() == ZX_ERR_PEER_CLOSED) {
            std::cerr
                << "[THRASHER_CLIENT] Hint: Is fuchsia.fxfs.BlobReader routed correctly to this component?"
                << std::endl;
          }
          logged_connection_error_ = true;
          should_call_done = true;
        }

        // Asynchronously calls the on_initialized_ callback on the configured dispatcher.
        // Ensures the callback is not invoked inline.
      }
      if (should_call_done) {
        AllGetVmosDone(result.status());
      }
      return;
    }
    if (result.value().is_error()) {
      std::cerr << "[THRASHER_CLIENT] GetVmo returned an error for " << merkle_root << ": "
                << static_cast<uint32_t>(result.value().error_value()) << std::endl;
      IssueGetVmo(index + 1);
      return;
    }

    zx::vmo vmo = std::move(result.value().value()->vmo);
    uint64_t size = 0;
    zx_status_t get_size_status = vmo.get_size(&size);

    if (get_size_status != ZX_OK) {
      std::cerr << "Failed to get VMO size for " << merkle_root << std::endl;
      IssueGetVmo(index + 1);
      return;
    }

    if (size == 0) {
      std::cerr << "VMO for " << merkle_root << " has zero size. Skipping." << std::endl;
      IssueGetVmo(index + 1);
      return;
    }

    uintptr_t mapped_addr;
    zx_status_t status = zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, vmo, 0, size, &mapped_addr);
    if (status != ZX_OK) {
      std::cerr << "Failed to map VMO for " << merkle_root << ": " << zx_status_get_string(status)
                << std::endl;
      IssueGetVmo(index + 1);
      return;
    }
    void* mapped = reinterpret_cast<void*>(mapped_addr);

    // Touch each page to ensure it's faulted in.
    volatile uint8_t value;
    const size_t page_size = zx_system_get_page_size();
    for (size_t i = 0; i < size; i += page_size) {
      value = static_cast<const uint8_t*>(mapped)[i];
    }
    (void)value;

    {
      std::lock_guard<std::mutex> lock(mutex_);
      total_allocated_bytes_ += size;
      mapped_blobs_.push_back({mapped, (size_t)size, std::move(vmo), merkle_root});
    }

    // Check limit *before* issuing the next call
    bool reached_limit = false;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      if (total_allocated_bytes_ > max_blob_size_bytes_) {
        reached_limit = true;
      }
    }

    if (reached_limit) {
      AllGetVmosDone(ZX_OK);
      return;
    }

    IssueGetVmo(index + 1);
  }

  void AllGetVmosDone(zx_status_t status) {
    if (on_initialized_) {
      {
        std::lock_guard<std::mutex> lock(mutex_);
        if (status != ZX_OK && !mapped_blobs_.empty()) {
          FX_LOGS(WARNING) << "Some blobs failed to load, but proceeding with "
                           << mapped_blobs_.size() << " blobs.";
          status = ZX_OK;
        }
      }
      on_initialized_(status);
      on_initialized_ = nullptr;
    }
  }

  void FinishInitialize(zx_status_t status) {
    async::PostTask(config_.dispatcher, [this, status, self = shared_from_this()]() {
      if (on_initialized_) {
        on_initialized_(status);
        on_initialized_ = nullptr;
      } else {
        FX_LOGS(WARNING) << "BlobThrasher::FinishInitialize on_initialized_ is null";
      }
    });
  }

  // Cleans up resources and invokes the final ThrashCallback.
  void FinishThrashing() {
    std::vector<zx::vmo> vmos;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      for (auto& blob : mapped_blobs_) {
        vmos.push_back(std::move(blob.vmo));
      }
      mapped_blobs_.clear();
      if (callback_) {
        async::PostTask(config_.dispatcher, [cb = callback_, vmos = std::move(vmos)]() mutable {
          (*cb)(std::move(vmos));
        });
      }
    }
  }

  ThrashConfig config_;
  std::vector<std::string> merkle_roots_;
  fidl::ClientEnd<fuchsia_fxfs::BlobReader> blob_reader_client_;
  std::optional<fidl::WireClient<ffxfs::BlobReader>> blob_reader_;
  std::vector<MappedBlob> mapped_blobs_;
  std::mutex mutex_;
  size_t total_allocated_bytes_ = 0;
  bool logged_connection_error_ = false;
  std::atomic<uint64_t> pages_touched_counter_{0};
  std::atomic<bool> keep_running_status_thread_{false};
  std::thread status_thread_;
  std::unique_ptr<PageTracker> page_tracker_;
  std::vector<size_t> blob_page_offsets_;
  fit::function<void(zx_status_t)> on_initialized_;
  std::vector<std::thread> worker_threads_;
  size_t max_blob_size_bytes_;
  std::shared_ptr<ThrashCallback> callback_;
  std::shared_ptr<StatusCallback> status_callback_;
  std::thread waiter_thread_;
};

std::shared_ptr<Thrasher> CreateAnonThrasher(ThrashConfig config, size_t buffer_size_bytes) {
  return std::make_shared<AnonThrasher>(std::move(config), buffer_size_bytes);
}

std::shared_ptr<Thrasher> CreateMmapThrasher(ThrashConfig config, std::string filename) {
  return std::make_shared<MmapThrasher>(std::move(config), std::move(filename));
}

std::shared_ptr<Thrasher> CreateDirThrasher(ThrashConfig config, std::string dirname) {
  return std::make_shared<DirThrasher>(std::move(config), std::move(dirname));
}

std::shared_ptr<Thrasher> CreateBlobThrasher(ThrashConfig config,
                                             const std::vector<std::string>& merkle_roots,
                                             size_t max_blob_size_bytes) {
  auto client_end = component::Connect<ffxfs::BlobReader>();
  if (!client_end.is_ok()) {
    FX_LOGS(ERROR) << "Failed to connect to BlobReader: " << client_end.status_string();
    return nullptr;
  }
  return std::make_shared<BlobThrasher>(std::move(config), std::move(client_end.value()),
                                        merkle_roots, max_blob_size_bytes);
}

std::shared_ptr<Thrasher> CreateBlobThrasherWithClient(
    ThrashConfig config, fidl::ClientEnd<ffxfs::BlobReader> client_end,
    const std::vector<std::string>& merkle_roots, size_t max_blob_size_bytes) {
  return std::make_shared<BlobThrasher>(std::move(config), std::move(client_end), merkle_roots,
                                        max_blob_size_bytes);
}

void LogVmos(const std::vector<zx::vmo>& vmos, bool reaccount_blob_vmos) {
  if (vmos.empty()) {
    return;
  }

  for (const auto& vmo : vmos) {
    print_vmo_info(vmo, "Blob");
  }
}
