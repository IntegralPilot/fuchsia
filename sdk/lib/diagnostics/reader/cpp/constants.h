// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DIAGNOSTICS_READER_CPP_CONSTANTS_H_
#define LIB_DIAGNOSTICS_READER_CPP_CONSTANTS_H_

namespace diagnostics::reader {

constexpr char kMonikerName[] = "moniker";
constexpr char kPayloadName[] = "payload";
constexpr char kVersionName[] = "version";
constexpr char kMetadataName[] = "metadata";
constexpr char kMetadataFilename[] = "filename";
constexpr char kMetadataNameOfInspectTree[] = "name";
constexpr char kMetadataComponentURL[] = "component_url";
constexpr char kMetadataTimestamp[] = "timestamp";
constexpr char kMetadataErrors[] = "errors";
constexpr char kMetadataErrorsMessage[] = "message";

constexpr char kMetadataFile[] = "file";
constexpr char kMetadataLine[] = "line";
constexpr char kMetadataPid[] = "pid";
constexpr char kMetadataSeverity[] = "severity";
constexpr char kMetadataTags[] = "tags";
constexpr char kMetadataTid[] = "tid";
constexpr char kPayloadRoot[] = "root";
constexpr char kPayloadMessageValue[] = "value";
constexpr char kPayloadKeys[] = "keys";

constexpr char kErrorDroppedLogs[] = "dropped_logs";
constexpr char kErrorRolledOutLogs[] = "rolled_out_logs";
constexpr char kErrorParseRecord[] = "parse_record";
constexpr char kErrorOther[] = "other";
constexpr char kMessage[] = "message";
constexpr char kCount[] = "count";

}  // namespace diagnostics::reader

#endif  // LIB_DIAGNOSTICS_READER_CPP_CONSTANTS_H_
