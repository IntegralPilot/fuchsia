// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/testing/fakes/data_provider.h"

#include <fuchsia/feedback/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

#include <vector>

#include "src/developer/forensics/utils/archive.h"
#include "src/lib/fsl/vmo/sized_vmo.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace forensics {
namespace fakes {
namespace {

using namespace fuchsia::feedback;

std::string AnnotationsToJSON(const std::vector<Annotation>& annotations) {
  std::string json = "{\n";
  for (const Annotation& annotation : annotations) {
    json +=
        fxl::StringPrintf("\t\"%s\": \"%s\"\n", annotation.key.c_str(), annotation.value.c_str());
  }
  json += "}\n";
  return json;
}

std::vector<Annotation> CreateAnnotations() {
  return {
      Annotation{.key = "annotation_key_1", .value = "annotation_value_1"},
      Annotation{.key = "annotation_key_2", .value = "annotation_value_2"},
      Annotation{.key = "annotation_key_3", .value = "annotation_value_3"},
  };
}

Attachment CreateSnapshot() {
  std::map<std::string, std::string> attachments;

  attachments["annotations.json"] = AnnotationsToJSON(CreateAnnotations());
  attachments["attachment_key"] = "attachment_value";

  fsl::SizedVmo archive;
  Archive(attachments, &archive);

  return {.key = "snapshot.zip", .value = std::move(archive).ToTransport()};
}

}  // namespace

void DataProvider::GetAnnotations(fuchsia::feedback::GetAnnotationsParameters params,
                                  GetAnnotationsCallback callback) {
  callback(std::move(Annotations().set_annotations2(CreateAnnotations())));
}

void DataProvider::GetSnapshot(fuchsia::feedback::GetSnapshotParameters parms,
                               GetSnapshotCallback callback) {
  callback(
      std::move(Snapshot().set_annotations2(CreateAnnotations()).set_archive(CreateSnapshot())));
}

}  // namespace fakes
}  // namespace forensics
