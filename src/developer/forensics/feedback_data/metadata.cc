// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/metadata.h"

#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <zircon/utc.h>

#include <optional>
#include <set>

#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/feedback_data/errors.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/lib/fxl/strings/split_string.h"
#include "third_party/rapidjson/include/rapidjson/document.h"
#include "third_party/rapidjson/include/rapidjson/prettywriter.h"
#include "third_party/rapidjson/include/rapidjson/rapidjson.h"
#include "third_party/rapidjson/include/rapidjson/stringbuffer.h"

namespace forensics {
namespace feedback_data {
namespace {

using namespace rapidjson;

std::set<std::string> kUtcBootDifferenceAllowlist = {
    kAttachmentInspect,
    kAttachmentLogKernel,
    kAttachmentLogSystem,
};

std::set<std::string> kPreviousBootUtcBootDifferenceAllowlist = {
    kAttachmentInspectPreviousBoot,
    kAttachmentLogKernelPrevious,
    kAttachmentLogSystemPrevious,
};

std::string ToString(const enum feedback::AttachmentState state) {
  switch (state) {
    case feedback::AttachmentState::kComplete:
      return "complete";
    case feedback::AttachmentState::kPartial:
      return "partial";
    case feedback::AttachmentState::kMissing:
      return "missing";
  }
}

// Create a complete list set of annotations from the collected annotations and the allowlist.
feedback::Annotations AllAnnotations(const std::set<std::string>& default_snapshot_annotations,
                                     const feedback::Annotations& annotations) {
  feedback::Annotations all_annotations = annotations;

  for (const std::string& key : default_snapshot_annotations) {
    if (!all_annotations.contains(key)) {
      // There is an annotation in the default list that was not produced by any provider.
      // Annotations that are excluded by a product should be marked as "not available in product,"
      // so this indicates a logical error on the Feedback-side.
      all_annotations.insert({key, ErrorOrString(Error::kLogicError)});
    }
  }

  return all_annotations;
}

// Create a complete list set of attachments from the collected attachments and the allowlist.
feedback::Attachments AllAttachments(const feedback::AttachmentKeys& allowlist,
                                     const feedback::Attachments& attachments) {
  feedback::Attachments all_attachments;

  // Because attachments can contain large blobs of text and we only care about the state of the
  // attachment, its associated error, and collection duration, we don't copy the value of the
  // attachment.
  for (const auto& [k, v] : attachments) {
    switch (v.State()) {
      case feedback::AttachmentState::kComplete:
        all_attachments.insert(
            {k, feedback::AttachmentValue("", v.CollectionDuration(), v.Metadata())});
        break;
      case feedback::AttachmentState::kPartial:
        all_attachments.insert(
            {k, feedback::AttachmentValue("", v.Error(), v.CollectionDuration(), v.Metadata())});
        break;
      case feedback::AttachmentState::kMissing:
        all_attachments.insert(
            {k, feedback::AttachmentValue(v.Error(), v.CollectionDuration(), v.Metadata())});
        break;
    }
  }

  for (const feedback::AttachmentKey& key : allowlist) {
    if (!all_attachments.contains(key)) {
      all_attachments.insert({key, feedback::AttachmentValue(Error::kLogicError, zx::duration(0))});
    }
  }

  return all_attachments;
}

void AddUtcBootDifference(const std::optional<zx::duration>& utc_boot_difference, Value* file,
                          Document::AllocatorType& allocator) {
  // TODO(https://fxbug.dev/360946313): change field name to utc_boot_difference_nanos.
  if (!utc_boot_difference.has_value() || !file->IsObject() ||
      file->HasMember("utc_monotonic_difference_nanos") ||
      (file->HasMember("state") && (*file)["state"].IsString() &&
       (*file)["state"].GetString() == ToString(feedback::AttachmentState::kMissing))) {
    return;
  }

  file->AddMember("utc_monotonic_difference_nanos",
                  Value().SetInt64(utc_boot_difference.value().get()), allocator);
}

void AddUtcBootDifferences(const std::optional<zx::duration> utc_boot_difference,
                           const std::optional<zx::duration> previous_boot_utc_boot_difference,
                           Document* metadata_json) {
  if (!metadata_json->HasMember("files")) {
    return;
  }

  for (auto& file : (*metadata_json)["files"].GetObject()) {
    if (kUtcBootDifferenceAllowlist.contains(file.name.GetString())) {
      AddUtcBootDifference(utc_boot_difference, &file.value, metadata_json->GetAllocator());
    }

    if (kPreviousBootUtcBootDifferenceAllowlist.contains(file.name.GetString())) {
      AddUtcBootDifference(previous_boot_utc_boot_difference, &file.value,
                           metadata_json->GetAllocator());
    }
  }
}

void AddAttachments(const feedback::AttachmentKeys& attachment_allowlist,
                    const feedback::Attachments& attachments, Document* metadata_json) {
  if (attachment_allowlist.empty()) {
    return;
  }

  auto& allocator = metadata_json->GetAllocator();
  auto MakeValue = [&allocator](const std::string& v) { return Value(v, allocator); };

  for (const auto& [name, v] : AllAttachments(attachment_allowlist, attachments)) {
    Value file(kObjectType);

    file.AddMember("state", MakeValue(ToString(v.State())), allocator);
    if (v.HasError()) {
      file.AddMember("error", MakeValue(ToReason(v.Error())), allocator);
    }
    file.AddMember("collection_duration_monotonic_nanos",
                   Value().SetInt64(v.CollectionDuration().get()), allocator);
    for (const auto& [meta_key, meta_val] : v.Metadata()) {
      file.AddMember(MakeValue(meta_key), MakeValue(meta_val), allocator);
    }

    (*metadata_json)["files"].AddMember(MakeValue(name), file, allocator);
  }
}

void AddAnnotationsJson(const std::set<std::string>& default_snapshot_annotations,
                        const feedback::Annotations& annotations,
                        const bool missing_non_platform_annotations, Document* metadata_json) {
  const feedback::Annotations all_annotations =
      AllAnnotations(default_snapshot_annotations, annotations);

  bool has_non_platform = all_annotations.size() > default_snapshot_annotations.size();
  if (default_snapshot_annotations.empty() &&
      !(has_non_platform || missing_non_platform_annotations)) {
    return;
  }

  auto& allocator = metadata_json->GetAllocator();
  auto MakeValue = [&allocator](const std::string& v) { return Value(v, allocator); };

  Value present(kArrayType);
  Value missing(kObjectType);

  size_t num_present_platform = 0u;
  size_t num_missing_platform = 0u;
  for (const auto& [k, v] : all_annotations) {
    if (!default_snapshot_annotations.contains(k)) {
      continue;
    }

    Value key(MakeValue(k));
    if (v.HasValue()) {
      present.PushBack(key, allocator);
      ++num_present_platform;
    } else {
      missing.AddMember(key, MakeValue(ToReason(v.Error())), allocator);
      ++num_missing_platform;
    }
  }

  if (has_non_platform || missing_non_platform_annotations) {
    if (!missing_non_platform_annotations) {
      present.PushBack("non-platform annotations", allocator);
    } else {
      missing.AddMember("non-platform annotations", "too many non-platfrom annotations added",
                        allocator);
    }
  }

  Value state;
  if (num_present_platform == default_snapshot_annotations.size() &&
      !missing_non_platform_annotations) {
    state = "complete";
  } else if (num_missing_platform == default_snapshot_annotations.size() && !has_non_platform &&
             missing_non_platform_annotations) {
    state = "missing";
  } else {
    state = "partial";
  }

  Value annotations_json(kObjectType);
  annotations_json.AddMember("state", state, allocator);
  annotations_json.AddMember("missing annotations", missing, allocator);
  annotations_json.AddMember("present annotations", present, allocator);

  (*metadata_json)["files"].AddMember("annotations.json", annotations_json, allocator);
}

void AddLogRedactionCanary(const std::string& log_redaction_canary, Document* metadata_json) {
  auto& allocator = metadata_json->GetAllocator();
  Value lines(kArrayType);
  for (const std::string& line :
       fxl::SplitStringCopy(log_redaction_canary, "\n", fxl::WhiteSpaceHandling::kTrimWhitespace,
                            fxl::SplitResult::kSplitWantNonEmpty)) {
    lines.PushBack(Value(line, allocator), allocator);
  }

  metadata_json->AddMember("log_redaction_canary", lines, allocator);
}

}  // namespace

Metadata::Metadata(async_dispatcher_t* dispatcher, timekeeper::Clock* clock,
                   UtcClockReadyWatcherBase* utc_clock_ready_watcher, RedactorBase* redactor,
                   const bool is_first_instance,
                   const std::set<std::string>& default_snapshot_annotations,
                   const feedback::AttachmentKeys& attachment_allowlist)
    : log_redaction_canary_(redactor->UnredactedCanary()),
      default_snapshot_annotations_(default_snapshot_annotations),
      attachment_allowlist_(attachment_allowlist),
      utc_provider_(utc_clock_ready_watcher, clock,
                    PreviousBootFile::FromCache(is_first_instance, kUtcBootDifferenceFile)) {
  redactor->Redact(log_redaction_canary_);
}

std::string Metadata::MakeMetadata(const feedback::Annotations& annotations,
                                   const feedback::Attachments& attachments,
                                   const std::string& snapshot_uuid,
                                   bool missing_non_platform_annotations) {
  Document metadata_json(kObjectType);
  auto& allocator = metadata_json.GetAllocator();

  auto MetadataString = [&metadata_json]() {
    StringBuffer buffer;
    PrettyWriter<StringBuffer> writer(buffer);

    metadata_json.Accept(writer);

    return std::string(buffer.GetString());
  };

  // Insert all top-level fields
  metadata_json.AddMember("snapshot_version", Value(SnapshotVersion::kString, allocator),
                          allocator);
  metadata_json.AddMember("metadata_version", Value(Metadata::kVersion, allocator), allocator);
  metadata_json.AddMember("snapshot_uuid", Value(snapshot_uuid, allocator), allocator);
  metadata_json.AddMember("files", Value(kObjectType), allocator);
  AddLogRedactionCanary(log_redaction_canary_, &metadata_json);

  const bool has_non_platform_annotations =
      annotations.size() > default_snapshot_annotations_.size();

  if (default_snapshot_annotations_.empty() && attachment_allowlist_.empty() &&
      !has_non_platform_annotations && !missing_non_platform_annotations) {
    return MetadataString();
  }

  AddAttachments(attachment_allowlist_, attachments, &metadata_json);
  AddAnnotationsJson(default_snapshot_annotations_, annotations, missing_non_platform_annotations,
                     &metadata_json);
  AddUtcBootDifferences(utc_provider_.CurrentUtcBootDifference(),
                        utc_provider_.PreviousBootUtcBootDifference(), &metadata_json);

  return MetadataString();
}

}  // namespace feedback_data
}  // namespace forensics
