// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_PERCPU_WRITER_INCLUDE_LIB_PERCPU_WRITER_BUFFER_H_
#define ZIRCON_KERNEL_LIB_PERCPU_WRITER_INCLUDE_LIB_PERCPU_WRITER_BUFFER_H_

#include <lib/fxt/interned_string.h>
#include <lib/fxt/record_types.h>
#include <lib/fxt/serializer.h>
#include <lib/fxt/string_ref.h>
#include <lib/fxt/thread_ref.h>
#include <lib/spsc_buffer/spsc_buffer.h>

#include <ktl/optional.h>

#include "arch/ops.h"
#include "kernel_aspace_allocator.h"
#include "platform/timer.h"

namespace percpu_writer {
using fxt::operator""_intern;

// percpu_writer::Buffer wraps an SpscBuffer and adds functionality to track dropped trace records.
class Buffer {
 public:
  using BufferImpl = SpscBuffer<KernelAspaceAllocator, const char*>;

  // LINT.IfChange(DroppedRecordStats)
  // This struct keeps track of the duration, number, and size of trace records dropped when the
  // buffer is full. These statistics are emitted to the trace buffer as a duration as soon as
  // space is available to do so, at which point the values are reset to 0, or false in the case
  // of has_dropped.
  struct DroppedRecordStats {
    zx_instant_boot_ticks_t first_dropped{0};
    zx_instant_boot_ticks_t last_dropped{0};

    // By storing num_dropped and bytes_dropped in 32-bit values, we ensure that they can each
    // be stored in a single 64-bit word in the FXT record we emit when space is available.
    uint32_t num_dropped{0};
    uint32_t bytes_dropped{0};
    bool has_dropped{false};

    void Reset() {
      first_dropped = 0;
      last_dropped = 0;
      num_dropped = 0;
      bytes_dropped = 0;
      has_dropped = false;
    }

    void Track(zx_instant_boot_ticks_t now, uint32_t size) {
      if (!has_dropped) {
        first_dropped = now;
        has_dropped = true;
      }
      last_dropped = now;
      num_dropped++;
      bytes_dropped += size;
    }

    bool HasDropped() const { return has_dropped; }
  };
  static_assert(sizeof(DroppedRecordStats) == 32);
  // LINT.ThenChange(//zircon/kernel/lib/ktrace/src/lib.rs:DroppedRecordStats)

  const DroppedRecordStats& drop_stats() const { return drop_stats_; }

  // Reservation encapsulates a pending write to the buffer.
  //
  // This class implements the fxt::Writer::Reservation trait, which is required by the FXT
  // serializer.
  //
  // It is absolutely imperative that interrupts remain disabled for the lifetime of this class.
  // Enabling interrupts at any point during the lifetime of this class will break the
  // single-writer invariant of each per-CPU buffer and lead to subtle concurrency bugs that may
  // manifest as corrupt trace data. Unfortunately, there is no way for us to programmatically
  // ensure this, so we do our best by asserting that interrupts are disabled in every method of
  // this class. It is therefore up to the caller to ensure that interrupts are disabled for the
  // lifetime of this object.
  class Reservation {
   public:
    ~Reservation() { DEBUG_ASSERT(arch_ints_disabled()); }

    // Disallow copies and move assignment, but allow moves.
    // Disallowing move assignment allows the saved interrupt state to be const.
    Reservation(const Reservation&) = delete;
    Reservation& operator=(const Reservation&) = delete;
    Reservation& operator=(Reservation&&) = delete;
    Reservation(Reservation&& other) : reservation_(ktl::move(other.reservation_)) {
      DEBUG_ASSERT(arch_ints_disabled());
    }

    void WriteWord(uint64_t word) {
      DEBUG_ASSERT(arch_ints_disabled());
      reservation_.Write(ktl::span<ktl::byte>(reinterpret_cast<ktl::byte*>(&word), sizeof(word)));
    }

    void WriteBytes(const void* bytes, size_t num_bytes) {
      DEBUG_ASSERT(arch_ints_disabled());
      // Write the data provided.
      reservation_.Write(
          ktl::span<const ktl::byte>(static_cast<const ktl::byte*>(bytes), num_bytes));

      // Write any padding bytes necessary.
      constexpr ktl::byte kZero[8]{};
      const size_t aligned_bytes = ROUNDUP(num_bytes, 8);
      const uint8_t num_zeros_to_write = static_cast<uint8_t>(aligned_bytes - num_bytes);
      if (num_zeros_to_write != 0) {
        reservation_.Write(ktl::span<const ktl::byte>(kZero, num_zeros_to_write));
      }
    }

    void Commit() {
      DEBUG_ASSERT(arch_ints_disabled());
      reservation_.Commit();
    }

    static zx::result<Reservation> FromSpscReservation(
        zx::result<BufferImpl::Reservation> reservation, uint64_t header) {
      if (reservation.is_error()) {
        return reservation.take_error();
      }
      return zx::ok(Reservation(ktl::move(reservation.value()), header));
    }

   private:
    friend class Buffer;
    Reservation(BufferImpl::Reservation reservation, uint64_t header)
        : reservation_(ktl::move(reservation)) {
      DEBUG_ASSERT(arch_ints_disabled());
      WriteWord(header);
    }

    BufferImpl::Reservation reservation_;
  };

  // Initializes the underlying SpscBuffer and metadata.
  zx_status_t Init(uint32_t size, const char* buffer_name,
                   fxt::ThreadRef<fxt::RefType::kInline> assigned_cpu_ref) {
    // Allocate the KOIDs used to annotate CPU trace records.
    cpu_ref_ = assigned_cpu_ref;
    size_ = size;
    return buffer_.Init(size, buffer_name);
  }

  // Drains the underlying SpscBuffer.
  void Drain() { buffer_.Drain(); }

  // Reads from the underlying SpscBuffer.
  template <CopyOutFunction CopyFunc>
  zx::result<uint32_t> Read(CopyFunc copy_fn, uint32_t len) {
    return buffer_.Read(copy_fn, len);
  }

  // Returns a pointer to the underlying SpscBuffer.
  BufferImpl* spsc_buffer() { return &buffer_; }

  // Returns a pointer to the dropped record statistics.
  DroppedRecordStats* drop_stats_ptr() { return &drop_stats_; }

  // We interpose ourselves in the Reserve path to ensure that we can emit a record containing
  // the dropped records statistics if we need to.
  zx::result<Reservation> Reserve(uint64_t header) {
    DEBUG_ASSERT(arch_ints_disabled());
    // Compute the number of bytes we need to reserve from the provided fxt header.
    const uint32_t num_words = fxt::RecordFields::Type::Get<uint32_t>(header) == 15
                                   ? fxt::LargeRecordFields::RecordSize::Get<uint32_t>(header)
                                   : fxt::RecordFields::RecordSize::Get<uint32_t>(header);
    const uint32_t size = num_words * sizeof(uint64_t);

    // If first_dropped_ is set to a value, then we are currently tracking a run of dropped trace
    // records, so we need to emit a duration record containing that information. We could emit
    // this record independently (with its own call to SpscBuffer::Reserve), but this could lead
    // to situations in which we thrash and emit multiple DroppedRecordDurationEvents in a row.
    // To avoid this, we attempt to reserve the desired size plus the size needed to store the
    // DroppedRecordDurationEvent record, and then write the statistics into the first part of
    // the reservation before returning it.
    uint32_t total_size = size;
    if (drop_stats_.HasDropped()) {
      total_size += sizeof(DroppedRecordDurationEvent);
    }

    zx::result<BufferImpl::Reservation> res = buffer_.Reserve(total_size);
    if (res.is_error()) {
      // If the reservation failed, then we did not have enough space in this buffer, and the
      // record we were attempting to write will be dropped. Add the "size" to the dropped record
      // statistics. Notably, we do not add the "total_size," because that may include the size
      // of the DroppedRecordDurationEvent.
      TrackDroppedRecord(size);
      return res.take_error();
    }

    // If we need to write a dropped record duration event, do that here.
    DEBUG_ASSERT(drop_stats_.HasDropped() || total_size == size);
    if (drop_stats_.HasDropped()) {
      DroppedRecordDurationEvent record = SerializeDropStats();
      res->Write(ktl::span<ktl::byte>(reinterpret_cast<ktl::byte*>(&record), sizeof(record)));
      // We've successfully written out the dropped record stats, so reset them for the next run.
      ResetDropStats();
    }
    return Reservation::FromSpscReservation(ktl::move(res), header);
  }

  // Emit the dropped record stats to the trace buffer.
  // If we're not tracking a run of dropped records, this is a no-op.
  zx_status_t EmitDropStats() {
    DEBUG_ASSERT(arch_ints_disabled());
    if (!drop_stats_.HasDropped()) {
      DEBUG_ASSERT(drop_stats_.last_dropped == 0);
      return ZX_OK;
    }

    // Try to reserve a slot for the duration record. This will fail if there still isn't enough
    // space in buffer to store the statistics.
    zx::result<BufferImpl::Reservation> res = buffer_.Reserve(sizeof(DroppedRecordDurationEvent));
    if (res.is_error()) {
      return res.status_value();
    }
    const DroppedRecordDurationEvent record = SerializeDropStats();

    ktl::span bytes =
        ktl::span<const ktl::byte>(reinterpret_cast<const ktl::byte*>(&record), sizeof(record));
    res->Write(bytes);
    res->Commit();

    // We've successfully emitted a record containing statistics on the last run of dropped
    // records. To prepare for the next run, we must reset the stats.
    ResetDropStats();
    return ZX_OK;
  }

  // Resets the dropped records statistics to their initial values.
  // This is used to clear the stats after they've been emitted to a trace buffer.
  void ResetDropStats() { drop_stats_.Reset(); }

  uint32_t Size() const { return size_; }

  // This is the structure of the FXT duration event that will store dropped record metadata in
  // the trace buffer. Normally, we would use the FXT serialization functions to build this record
  // dynamically, but we cannot do this in the PerCpuBuffer because those functions invoke
  // writer->Reserve, which would lead to recursion. To avoid this, we serialize the record
  // manually using this struct, which in turn is set up to match the structure outlined in the
  // FXT spec: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#event-record
  //
  // Eventually, it would be nice to have the FXT serialization library support in-place
  // serialization, as that would allow us to remove this bespoke functionality.
  // LINT.IfChange(DroppedRecordDurationEvent)
  struct DroppedRecordDurationEvent {
    uint64_t header;
    zx_instant_boot_ticks_t start;
    uint64_t process_id;
    uint64_t thread_id;
    uint64_t num_dropped_arg;
    uint64_t bytes_dropped_arg;
    zx_instant_boot_ticks_t end;
  };
  static_assert(ktl::is_standard_layout_v<DroppedRecordDurationEvent>);
  // LINT.ThenChange(//zircon/kernel/lib/ktrace/src/lib.rs:DroppedRecordDurationEvent)

  // Serializes the dropped record statistics into a DroppedRecordDurationEvent.
  DroppedRecordDurationEvent SerializeDropStats() const {
    // This method should only be called if we are currently tracking a run of dropped records.
    DEBUG_ASSERT(drop_stats_.HasDropped());
    DEBUG_ASSERT(drop_stats_.last_dropped != 0);

    constexpr fxt::WordSize record_size =
        fxt::WordSize::FromBytes(sizeof(DroppedRecordDurationEvent));
    const fxt::StringRef<fxt::RefType::kId> name_ref = fxt::StringRef{"drop_stats"_intern};
    const fxt::StringRef<fxt::RefType::kId> category_ref = fxt::StringRef{"kernel:meta"_intern};
    constexpr uint64_t num_args = 2;
    const fxt::Argument num_dropped_arg =
        fxt::Argument{"num_records"_intern, drop_stats_.num_dropped};
    const fxt::Argument bytes_dropped_arg =
        fxt::Argument{"num_bytes"_intern, drop_stats_.bytes_dropped};
    const uint64_t header =
        fxt::MakeHeader(fxt::RecordType::kEvent, record_size) |
        fxt::EventRecordFields::EventType::Make(
            ToUnderlyingType(fxt::EventType::kDurationComplete)) |
        fxt::EventRecordFields::ArgumentCount::Make(num_args) |
        fxt::EventRecordFields::ThreadRef::Make(cpu_ref_.HeaderEntry()) |
        fxt::EventRecordFields::CategoryStringRef::Make(category_ref.HeaderEntry()) |
        fxt::EventRecordFields::NameStringRef::Make(name_ref.HeaderEntry());

    return {
        .header = header,
        .start = drop_stats_.first_dropped,
        .process_id = cpu_ref_.process().koid,
        .thread_id = cpu_ref_.thread().koid,
        .num_dropped_arg = num_dropped_arg.Header(),
        .bytes_dropped_arg = bytes_dropped_arg.Header(),
        .end = drop_stats_.last_dropped,
    };
  }

  // Adds a dropped record of the given size to the tracked statistics.
  void TrackDroppedRecord(uint32_t size) { drop_stats_.Track(current_boot_ticks(), size); }

  // The underlying SpscBuffer.
  BufferImpl buffer_;

 private:
  // Track stats of trace records dropped when the buffer is full.
  DroppedRecordStats drop_stats_;
  uint32_t size_{0};
  fxt::ThreadRef<fxt::RefType::kInline> cpu_ref_{0, 0};
};

}  // namespace percpu_writer

#endif  // ZIRCON_KERNEL_LIB_PERCPU_WRITER_INCLUDE_LIB_PERCPU_WRITER_BUFFER_H_
