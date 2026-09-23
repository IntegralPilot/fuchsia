// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_CONSTANTS_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_CONSTANTS_H_

#include <lib/zx/time.h>

#include "src/developer/forensics/utils/cobalt/metrics.h"
#include "src/developer/forensics/utils/storage_size.h"

namespace forensics {
namespace feedback_data {

///////////////////////////////////////////////////////////////////////////////////////////////////
// Attachments
///////////////////////////////////////////////////////////////////////////////////////////////////

// Platform attachments keys.
constexpr char kAttachmentAnnotations[] = "annotations.json";
constexpr char kAttachmentInspect[] = "inspect.json";
constexpr char kAttachmentInspectPreviousBoot[] = "inspect.previous_boot.json";
constexpr char kAttachmentKernelBootOptions[] = "build.kernel-boot-options.txt";
constexpr char kAttachmentLogKernel[] = "log.kernel.txt";
constexpr char kAttachmentLogKernelPrevious[] = "log.kernel.previous_boot.txt";
constexpr char kAttachmentLogSystem[] = "log.system.txt";
constexpr char kAttachmentLogSystemPrevious[] = "log.system.previous_boot.txt";
constexpr char kAttachmentMetadata[] = "metadata.json";
constexpr char kAttachmentProcessTree[] = "process_tree.txt";

// Attachment metadata.
constexpr char kAttachmentMetadataSourceKey[] = "source";
constexpr char kAttachmentMetadataSourceDisk[] = "disk";
constexpr char kAttachmentMetadataSourceStream[] = "stream";

// Snapshot key.
constexpr char kSnapshotFilename[] = "snapshot.zip";

// Other reserved attachment or file names.
constexpr char kMinidumpFilename[] = "minidump.dmp";
constexpr char kMinidumpUploadKey[] = "uploadFileMinidump";
constexpr char kSnapshotUuidFilename[] = "snapshot_uuid.txt";

///////////////////////////////////////////////////////////////////////////////////////////////////
// UTC-monotonic difference
///////////////////////////////////////////////////////////////////////////////////////////////////

// TODO(https://fxbug.dev/360946313): change to current_utc_boot_difference.txt.
constexpr char kUtcBootDifferenceFile[] = "current_utc_monotonic_difference.txt";

///////////////////////////////////////////////////////////////////////////////////////////////////
// Logs from current boot.
///////////////////////////////////////////////////////////////////////////////////////////////////

// Buffer up to 4MiB of logs in memory.
constexpr StorageSize kCurrentLogBufferSize = StorageSize::Megabytes(4);

// Buffer up to 8MiB of logs in memory when collecting a fallback snapshot from Archivist.
constexpr StorageSize kFallbackLogBufferSize = StorageSize::Megabytes(8);

// Stream and buffer logs for 5 minutes after a snapshot collected.
//
// TODO(https://fxbug.dev/42181644): Set the default value for userdebug to at least 1 hour. Logs
// will be streamed indefinitely because of hourly snapshots.
constexpr zx::duration kActiveLoggingPeriod = zx::min(5);

// At most 16KB of logs will be persisted each second.
constexpr StorageSize kMaxWriteSize = StorageSize::Kilobytes(16);

// Repeated messge format
constexpr char kRepeatedStrPrefix[] = "!!! MESSAGE REPEATED ";
constexpr char kRepeatedStrSuffix[] = " MORE TIMES !!!\n";
constexpr char kRepeatedOnceFormatStr[] = "!!! MESSAGE REPEATED 1 MORE TIME !!!\n";
constexpr char kRepeatedFormatStr[] = "!!! MESSAGE REPEATED %lu MORE TIMES !!!\n";

// One repeated message can occupy up to "kMaxRepeatedBuffers" buffers.
constexpr size_t kMaxRepeatedBuffers = 30;

// The current version of the snapshot. Update these values together!
struct SnapshotVersion {
  static constexpr cobalt::SnapshotVersion kCobalt = cobalt::SnapshotVersion::kV_01;
  static constexpr const char* kString = "1";
};

// The name of the protocol to use to read Feedback data from the Archive.
constexpr char kArchiveAccessorName[] = "fuchsia.diagnostics.ArchiveAccessor.feedback";

}  // namespace feedback_data
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_CONSTANTS_H_
