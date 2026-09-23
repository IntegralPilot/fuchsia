// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_BOARD_INFO_PROVIDER_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_BOARD_INFO_PROVIDER_H_

#include <fidl/fuchsia.hwinfo/cpp/fidl.h>

#include "src/developer/forensics/feedback/annotations/fidl_provider.h"
#include "src/developer/forensics/feedback/annotations/types.h"

namespace forensics::feedback {
namespace internal {

inline auto GetBoardInfo(fidl::Client<fuchsia_hwinfo::Board>& client) { return client->GetInfo(); }

}  // namespace internal

struct BoardInfoToAnnotations {
  Annotations operator()(const fuchsia_hwinfo::BoardGetInfoResponse& response);
  Annotations operator()(Error error);
};

// Responsible for collecting annotations for fuchsia.hwinfo/Board.
class BoardInfoProvider
    : public StaticSingleFidlMethodAnnotationProvider<
          fuchsia_hwinfo::Board, &internal::GetBoardInfo, BoardInfoToAnnotations> {
 public:
  using StaticSingleFidlMethodAnnotationProvider::StaticSingleFidlMethodAnnotationProvider;

  virtual ~BoardInfoProvider() = default;

  static std::set<std::string> GetAnnotationKeys();
  std::set<std::string> GetKeys() const override;
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ANNOTATIONS_BOARD_INFO_PROVIDER_H_
