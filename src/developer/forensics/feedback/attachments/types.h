// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_TYPES_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_TYPES_H_

#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <map>
#include <memory>
#include <set>
#include <string>
#include <string_view>

#include "src/developer/forensics/utils/errors.h"

namespace forensics::feedback {

using AttachmentKey = std::string;
using AttachmentKeys = std::set<AttachmentKey>;
using AttachmentMetadata = std::map<std::string, std::string>;

enum class AttachmentState {
  kComplete,
  kPartial,
  kMissing,
};

class AttachmentData {
 public:
  explicit AttachmentData(std::string value, AttachmentMetadata metadata = {})
      : state_(AttachmentState::kComplete),
        value_(std::make_unique<std::string>(std::move(value))),
        error_(std::nullopt),
        metadata_(std::move(metadata)) {}
  AttachmentData(std::string value, enum Error error, AttachmentMetadata metadata = {})
      : state_(AttachmentState::kPartial),
        value_(std::make_unique<std::string>(std::move(value))),
        error_(error),
        metadata_(std::move(metadata)) {}
  explicit AttachmentData(enum Error error, AttachmentMetadata metadata = {})
      : state_(AttachmentState::kMissing),
        value_(nullptr),
        error_(error),
        metadata_(std::move(metadata)) {}

  bool HasValue() const { return value_ != nullptr; }

  std::string_view Value() const {
    FX_CHECK(HasValue());
    return *value_;
  }

  bool HasError() const { return error_.has_value(); }

  enum Error Error() const {
    FX_CHECK(HasError());
    return error_.value();
  }

  AttachmentState State() const { return state_; }

  const AttachmentMetadata& Metadata() const { return metadata_; }

  AttachmentData Clone() const {
    if (HasValue() && HasError()) {
      return AttachmentData(*value_, *error_, metadata_);
    }

    if (HasValue()) {
      return AttachmentData(*value_, metadata_);
    }

    return AttachmentData(*error_, metadata_);
  }

 private:
  AttachmentState state_;
  std::unique_ptr<std::string> value_;
  std::optional<enum Error> error_;
  AttachmentMetadata metadata_;
};

class AttachmentValue {
 public:
  AttachmentValue(AttachmentData data, zx::duration collection_duration)
      : data_(std::move(data)), collection_duration_(collection_duration) {}
  AttachmentValue(std::string value, zx::duration collection_duration,
                  AttachmentMetadata metadata = {})
      : AttachmentValue(AttachmentData(std::move(value), std::move(metadata)),
                        collection_duration) {}
  AttachmentValue(std::string value, enum Error error, zx::duration collection_duration,
                  AttachmentMetadata metadata = {})
      : AttachmentValue(AttachmentData(std::move(value), error, std::move(metadata)),
                        collection_duration) {}
  AttachmentValue(enum Error error, zx::duration collection_duration,
                  AttachmentMetadata metadata = {})
      : AttachmentValue(AttachmentData(error, std::move(metadata)), collection_duration) {}

  bool HasValue() const { return data_.HasValue(); }

  std::string_view Value() const { return data_.Value(); }

  bool HasError() const { return data_.HasError(); }

  enum Error Error() const { return data_.Error(); }

  AttachmentState State() const { return data_.State(); }

  const AttachmentMetadata& Metadata() const { return data_.Metadata(); }

  zx::duration CollectionDuration() const { return collection_duration_; }

  // Allow callers to explicitly copy an attachment.
  AttachmentValue Clone() const { return AttachmentValue(data_.Clone(), collection_duration_); }

 private:
  AttachmentData data_;
  zx::duration collection_duration_;
};

using Attachment = std::pair<AttachmentKey, AttachmentValue>;
using Attachments = std::map<AttachmentKey, AttachmentValue>;

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_TYPES_H_
