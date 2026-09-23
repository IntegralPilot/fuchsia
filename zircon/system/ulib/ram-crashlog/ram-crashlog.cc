// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/cksum.h>
#include <stdint.h>
#include <string.h>
#include <zircon/boot/crash-reason.h>
#include <zircon/errors.h>

#include <algorithm>
#include <span>

#include <ram-crashlog/ram-crashlog.h>

#if _KERNEL
// to get arch_clean_cache_range()
#include <arch/ops.h>
#endif

namespace {
// When this module is used in the actual kernel, we need to make certain
// to actually clean the cache at very specific points in a crashlog update
// sequence.  If this is being built for user-mode, then the module is only
// being built for testing, and cache scrubbing is not needed.
#if _KERNEL
void clean_cache_range(void* ptr, size_t len) {
  arch_clean_cache_range(reinterpret_cast<uintptr_t>(ptr), len);
}
#else
void clean_cache_range(void* ptr, size_t len) {}
#endif

template <typename HdrType>
zx_status_t recover_crashlog(const HdrType& hdr, std::span<const uint8_t> payload,
                             recovered_ram_crashlog_t* log_out) {
  // Validate the header CRC. If this does not check out, then we cannot recover the log.
  const uint32_t expected_hdr_crc32 =
      crc32(0, reinterpret_cast<const uint8_t*>(&hdr), offsetof(HdrType, header_crc32));
  if (expected_hdr_crc32 != hdr.header_crc32) {
    return ZX_ERR_IO_DATA_INTEGRITY;
  }

  // Figure out the runtime. If the HdrType is ram_crashlog_header_v0_t, then it does not contain a
  // runtime field, so we just set runtime to uptime. This is valid because the monotonic and boot
  // timelines will not diverge in a system using v0 headers.
  const zx_instant_mono_t runtime = [&]() -> zx_instant_mono_t {
    if constexpr (std::is_same_v<HdrType, ram_crashlog_header_v0_t>) {
      return hdr.uptime;
    } else {
      return hdr.runtime;
    }
  }();

  // Validate the payload CRC32. We do not return an error if the CRC fails to match, we just make
  // a note that the payload is not valid.
  const size_t payload_length = std::min<size_t>(payload.size_bytes(), hdr.payload_len);
  const uint8_t* payload_ptr = payload_length > 0 ? payload.data() : nullptr;
  const uint32_t expected_payload_crc32 = crc32(0, payload_ptr, payload_length);
  const bool payload_valid =
      (payload_length == hdr.payload_len) && (expected_payload_crc32 == hdr.payload_crc32);

  log_out->uptime = hdr.uptime;
  log_out->runtime = runtime;
  log_out->reason = hdr.reason;
  log_out->payload_valid = payload_valid;
  log_out->payload = payload_ptr;
  log_out->payload_len = static_cast<uint32_t>(payload_length);

  return ZX_OK;
}

}  // namespace

zx_status_t ram_crashlog_stow(void* buf, size_t buf_len, const void* payload, uint32_t payload_len,
                              zircon_crash_reason_t sw_reason, zx_instant_boot_t uptime,
                              zx_instant_mono_t runtime) {
  // The user needs to provide a valid buffer to stow the log, but they do not
  // have to provide a payload.  That said, they may not provide a null payload
  // pointer and a non-zero payload length.
  if ((buf == nullptr) || ((payload == nullptr) && (payload_len != 0))) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Is the payload pointer located within the crashlog buffer?  If so, it
  // *must* be located immediately after the header, or we will consider it to
  // be an error.
  const uintptr_t payload_offset =
      reinterpret_cast<uintptr_t>(payload) - reinterpret_cast<uintptr_t>(buf);
  const bool payload_located_in_buffer = ((payload >= buf) && (payload_offset < buf_len));
  if ((payload_located_in_buffer) && (payload_offset != sizeof(ram_crashlog_v1_t))) {
    return ZX_ERR_INVALID_ARGS;
  }

  // We cannot stow a crashlog if the buffer provided to us is too small.  It
  // has to be large enough to hold the common payload structure, at a minimum.
  if (buf_len < sizeof(ram_crashlog_v1_t)) {
    return ZX_ERR_BUFFER_TOO_SMALL;
  }

  // Check the reboot reason to make sure it is valid.  Invalid reasons must be rejected.
  switch (sw_reason) {
    case ZirconCrashReason::Unknown:
    case ZirconCrashReason::NoCrash:
    case ZirconCrashReason::Oom:
    case ZirconCrashReason::Panic:
    case ZirconCrashReason::SoftwareWatchdog:
    case ZirconCrashReason::UserspaceRootJobTermination:
      break;
    default:
      return ZX_ERR_INVALID_ARGS;
  }

  // Figure out how much space we have for payload.  It is not an error for the
  // user to attempt to store more payload than we have room for, but we have to
  // truncate the payload in this situation.
  size_t max_payload = buf_len - sizeof(ram_crashlog_v1_t);
  if (payload_len > max_payload) {
    payload_len = static_cast<uint32_t>(max_payload);
  }

  // Great!  Time to get to work.  Start by figuring out which instance of the
  // header we should eventually occupy.
  ram_crashlog_v1_t& log = *(reinterpret_cast<ram_crashlog_v1_t*>(buf));
  uint8_t* tgt_payload = reinterpret_cast<uint8_t*>(&log + 1);
  uint64_t next_magic;
  ram_crashlog_header_v1_t* hdr;

  if (log.magic == RAM_CRASHLOG_MAGIC_2) {
    next_magic = RAM_CRASHLOG_MAGIC_3;
    hdr = &log.hdr[1];
  } else {
    next_magic = RAM_CRASHLOG_MAGIC_2;
    hdr = &log.hdr[0];
  }

  // Now fill out the header we chose to overwrite, computing the payload
  // CRC in the process.
  hdr->uptime = uptime;
  hdr->runtime = runtime;
  hdr->reason = sw_reason;
  hdr->payload_len = payload_len;
  hdr->payload_crc32 = crc32(0, reinterpret_cast<const uint8_t*>(payload), payload_len);

  // Compute the header CRC, then make sure it has been flushed all of the way
  // to RAM.
  hdr->header_crc32 = crc32(0, reinterpret_cast<const uint8_t*>(hdr),
                            offsetof(ram_crashlog_header_v1_t, header_crc32));
  clean_cache_range(hdr, sizeof(*hdr));

  // If we have a payload, and it has not already been directly rendered into
  // the crashlog buffer by the caller, copy the payload into place.  Then make
  // sure that it has been written all of the way out to physical RAM.  The old
  // header is active at this point.  If we had a non-empty payload previously
  // to this, we are almost certainly going to fail to recover the payload if we
  // were to spontaneously reboot right at this instant.  That said, we will
  // still attempt to recover whatever the old header said was there, so
  // hopefully we will end up getting something out of this.
  if (payload_len > 0) {
    if (!payload_located_in_buffer) {
      memcpy(tgt_payload, payload, payload_len);
    }
    clean_cache_range(tgt_payload, payload_len);
  }

  // Finally, toggle the magic number value in order to activate our new header.
  log.magic = next_magic;
  clean_cache_range(&log.magic, sizeof(log.magic));

  return ZX_OK;
}

zx_status_t ram_crashlog_recover(const void* buf, size_t buf_len,
                                 recovered_ram_crashlog_t* log_out) {
  // We cannot recover a crashlog if there is no place to get the crashlog from,
  // or no place to put the results.
  if ((buf == nullptr) || (log_out == nullptr)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // We cannot recover a crashlog if the buffer in which the crashlog is supposed to exist is too
  // small. Note that we may pass this check and still have a buffer that's too small if we are
  // recovering a v1 crashlog, so we have another check for that below.
  if (buf_len < sizeof(ram_crashlog_v0_t)) {
    return ZX_ERR_BUFFER_TOO_SMALL;
  }

  // Check the magic number at the start of the buffer to figure out which header version we should
  // use.
  const uint64_t magic = *(reinterpret_cast<const uint64_t*>(buf));
  switch (magic) {
    case RAM_CRASHLOG_MAGIC_0: {
      const ram_crashlog_v0_t* log = static_cast<const ram_crashlog_v0_t*>(buf);
      std::span<const uint8_t> payload{reinterpret_cast<const uint8_t*>(log + 1),
                                       buf_len - sizeof(*log)};
      return recover_crashlog(log->hdr[0], payload, log_out);
    }
    case RAM_CRASHLOG_MAGIC_1: {
      const ram_crashlog_v0_t* log = static_cast<const ram_crashlog_v0_t*>(buf);
      std::span<const uint8_t> payload{reinterpret_cast<const uint8_t*>(log + 1),
                                       buf_len - sizeof(*log)};
      return recover_crashlog(log->hdr[1], payload, log_out);
    }
    case RAM_CRASHLOG_MAGIC_2: {
      if (buf_len < sizeof(ram_crashlog_v1_t)) {
        return ZX_ERR_BUFFER_TOO_SMALL;
      }
      const ram_crashlog_v1_t* log = static_cast<const ram_crashlog_v1_t*>(buf);
      std::span<const uint8_t> payload{reinterpret_cast<const uint8_t*>(log + 1),
                                       buf_len - sizeof(*log)};
      return recover_crashlog(log->hdr[0], payload, log_out);
    }
    case RAM_CRASHLOG_MAGIC_3: {
      if (buf_len < sizeof(ram_crashlog_v1_t)) {
        return ZX_ERR_BUFFER_TOO_SMALL;
      }
      const ram_crashlog_v1_t* log = static_cast<const ram_crashlog_v1_t*>(buf);
      std::span<const uint8_t> payload{reinterpret_cast<const uint8_t*>(log + 1),
                                       buf_len - sizeof(*log)};
      return recover_crashlog(log->hdr[1], payload, log_out);
    }
    default:
      return ZX_ERR_IO_DATA_INTEGRITY;
  }
}
