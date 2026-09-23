// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/crash_reports/snapshot_store.h"

#include <fstream>
#include <string>
#include <utility>

#include "src/developer/forensics/crash_reports/constants.h"
#include "src/developer/forensics/crash_reports/item_location.h"
#include "src/developer/forensics/crash_reports/snapshot.h"
#include "src/developer/forensics/feedback/annotations/constants.h"

namespace forensics::crash_reports {
namespace {

// Helper function to make a shared_ptr from a rvalue-reference of a type.
template <typename T>
std::shared_ptr<T> MakeShared(T&& t) {
  return std::make_shared<T>(static_cast<std::remove_reference_t<T>&&>(t));
}

}  // namespace

SnapshotStore::SnapshotStore(feedback::AnnotationManager* annotation_manager,
                             std::string garbage_collected_snapshots_path,
                             const std::optional<SnapshotPersistence::Root>& temp_root,
                             const std::optional<SnapshotPersistence::Root>& persistent_root,
                             StorageSize max_archives_size)
    : annotation_manager_(annotation_manager),
      garbage_collected_snapshots_path_(std::move(garbage_collected_snapshots_path)),
      persistence_(temp_root, persistent_root),
      max_archives_size_(max_archives_size),
      current_archives_size_(0u),
      garbage_collected_snapshot_(
          kGarbageCollectedSnapshotUuid,
          feedback::Annotations({
              {feedback::kDebugSnapshotErrorKey, ErrorOrString("garbage collected")},
              {feedback::kDebugSnapshotPresentKey, ErrorOrString("false")},
          })),
      not_persisted_snapshot_(
          kNotPersistedSnapshotUuid,
          feedback::Annotations({
              {feedback::kDebugSnapshotErrorKey, ErrorOrString("not persisted")},
              {feedback::kDebugSnapshotPresentKey, ErrorOrString("false")},
          })),
      shutdown_snapshot_(kShutdownSnapshotUuid,
                         feedback::Annotations({
                             {feedback::kDebugSnapshotErrorKey, ErrorOrString("system shutdown")},
                             {feedback::kDebugSnapshotPresentKey, ErrorOrString("false")},
                         })),
      no_uuid_snapshot_(kNoUuidSnapshotUuid,
                        feedback::Annotations({
                            {feedback::kDebugSnapshotErrorKey, ErrorOrString("missing uuid")},
                            {feedback::kDebugSnapshotPresentKey, ErrorOrString("false")},
                        })) {
  // Load the file lines into a set of UUIDs.
  std::ifstream file(garbage_collected_snapshots_path_);
  for (std::string uuid; getline(file, uuid);) {
    garbage_collected_snapshots_.insert(uuid);
  }
}

Snapshot SnapshotStore::GetSnapshot(const std::string& uuid) {
  auto BuildMissing = [this](const SpecialCaseSnapshot& special_case) {
    return MissingSnapshot(annotation_manager_->ImmediatelyAvailable(), special_case.annotations);
  };

  if (uuid == kGarbageCollectedSnapshotUuid) {
    return BuildMissing(garbage_collected_snapshot_);
  }

  if (uuid == kNotPersistedSnapshotUuid) {
    return BuildMissing(not_persisted_snapshot_);
  }

  if (uuid == kShutdownSnapshotUuid) {
    return BuildMissing(shutdown_snapshot_);
  }

  if (uuid == kNoUuidSnapshotUuid) {
    return BuildMissing(no_uuid_snapshot_);
  }

  SnapshotData* data = FindSnapshotData(uuid);

  if (!data) {
    if (garbage_collected_snapshots_.contains(uuid)) {
      return BuildMissing(garbage_collected_snapshot_);
    }

    if (std::optional<ManagedSnapshot::Archive> snapshot = persistence_.Get(uuid);
        snapshot.has_value()) {
      return ManagedSnapshot::StoreShared(MakeShared(std::move(*snapshot)));
    }

    return BuildMissing(not_persisted_snapshot_);
  }

  return ManagedSnapshot::StoreWeak(data->archive);
}

std::vector<std::string> SnapshotStore::GetSnapshotUuids() const {
  return persistence_.GetSnapshotUuids();
}

MissingSnapshot SnapshotStore::GetMissingSnapshot(const std::string& uuid) {
  const Snapshot snapshot = GetSnapshot(uuid);
  FX_CHECK(std::holds_alternative<MissingSnapshot>(snapshot));

  return std::get<MissingSnapshot>(snapshot);
}

void SnapshotStore::DeleteSnapshot(const std::string& uuid) {
  if (persistence_.Contains(uuid)) {
    persistence_.Delete(uuid);
    return;
  }

  SnapshotData* data = FindSnapshotData(uuid);

  // The snapshot was likely dropped due to size constraints.
  if (!data) {
    return;
  }

  DropArchive(data);
  RecordAsGarbageCollected(uuid);
  data_.erase(uuid);
  insertion_order_.erase(std::remove(insertion_order_.begin(), insertion_order_.end(), uuid),
                         insertion_order_.end());
}

void SnapshotStore::DeleteAll() {
  // The iterator in a range-based for loop would be invalidated by calls to |data_.erase|, causing
  // bad behavior or even a crash. Use a while loop instead that grabs a new, valid iterator for
  // each iteration.
  while (data_.begin() != data_.end()) {
    // Grab copy of uuid to avoid asan failure when DeleteSnapshot erases |data_.begin()|.
    const std::string uuid = data_.begin()->first;
    DeleteSnapshot(uuid);
  }

  persistence_.DeleteAll();
}

void SnapshotStore::AddSnapshot(const std::string& uuid, fuchsia::feedback::Attachment archive) {
  FX_CHECK(!SnapshotExists(uuid)) << "Duplicate snapshot uuid '" << uuid << "' added to store";

  SnapshotData& data = data_[uuid];

  if (!archive.key.empty() && archive.value.vmo.is_valid()) {
    data.archive_size += StorageSize::Bytes(archive.key.size());
    data.archive_size += StorageSize::Bytes(archive.value.size);

    if (data.archive_size > max_archives_size_) {
      // Attempting to add to the store would needlessly garbage collect all snapshots despite
      // failing to add.
      FX_LOGS(WARNING) << "Snapshot for uuid '" << uuid << "' with a size of '"
                       << data.archive_size.Get() << "' is larger than the store max";
      data_.erase(uuid);
      return;
    }

    current_archives_size_ += data.archive_size;

    data.archive = MakeShared(ManagedSnapshot::Archive(archive));
  }

  insertion_order_.push_back(uuid);

  while (!insertion_order_.empty() && SizeLimitsExceeded()) {
    // We erase snapshots from |insertion_order_| when they get moved to disk.
    FX_CHECK(SnapshotLocation(insertion_order_.front()) == ItemLocation::kMemory)
        << "Snapshot for uuid " << insertion_order_.front() << " doesn't exist in memory";

    EnforceSizeLimits(insertion_order_.front());
    insertion_order_.pop_front();
  }
}

void SnapshotStore::EnforceSizeLimits(const std::string& uuid) {
  SnapshotData* data = FindSnapshotData(uuid);
  FX_CHECK(data);

  // Drop |data| if necessary.
  if (current_archives_size_ > max_archives_size_) {
    DropArchive(data);
    RecordAsGarbageCollected(uuid);
    data_.erase(uuid);
  }
}

ItemLocation SnapshotStore::MoveToPersistence(const std::string& uuid,
                                              const bool only_consider_tmp) {
  SnapshotData* data = FindSnapshotData(uuid);
  FX_CHECK(data);

  const std::optional<ItemLocation> location =
      persistence_.Add(uuid, *data->archive, data->archive_size, only_consider_tmp);

  if (!location.has_value()) {
    return ItemLocation::kMemory;
  }

  // Snapshot successfully moved to disk; no longer needed in memory.
  insertion_order_.erase(std::remove(insertion_order_.begin(), insertion_order_.end(), uuid),
                         insertion_order_.end());
  DropArchive(data);
  data_.erase(uuid);
  return *location;
}

void SnapshotStore::MoveToTmp(const std::string& uuid) { return persistence_.MoveToTmp(uuid); }

bool SnapshotStore::SnapshotExists(const std::string& uuid) {
  if (FindSnapshotData(uuid) != nullptr) {
    return true;
  }

  // Snapshot not in memory; check disc.
  return persistence_.Contains(uuid);
}

std::optional<ItemLocation> SnapshotStore::SnapshotLocation(const std::string& uuid) {
  if (FindSnapshotData(uuid) != nullptr) {
    return ItemLocation::kMemory;
  }

  // Snapshot not in memory; check disc.
  return persistence_.SnapshotLocation(uuid);
}

size_t SnapshotStore::Size() const { return data_.size(); }

bool SnapshotStore::IsGarbageCollected(const std::string& uuid) const {
  return garbage_collected_snapshots_.contains(uuid);
}

bool SnapshotStore::SizeLimitsExceeded() const {
  return current_archives_size_ > max_archives_size_;
}

void SnapshotStore::DropArchive(SnapshotData* data) {
  data->archive = nullptr;

  current_archives_size_ -= data->archive_size;
  data->archive_size = StorageSize::Bytes(0u);
}

void SnapshotStore::RecordAsGarbageCollected(const std::string& uuid) {
  if (garbage_collected_snapshots_.contains(uuid)) {
    return;
  }

  garbage_collected_snapshots_.insert(uuid);

  // Append the UUID to the file on its own line.
  std::ofstream file(garbage_collected_snapshots_path_, std::ofstream::out | std::ofstream::app);
  file << uuid << "\n";
  file.close();
}

SnapshotStore::SnapshotData* SnapshotStore::FindSnapshotData(const std::string& uuid) {
  return data_.contains(uuid) ? &(data_.at(uuid)) : nullptr;
}

}  // namespace forensics::crash_reports
