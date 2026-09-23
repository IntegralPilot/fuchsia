// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_TESTS_FIDL_DYNSUITE_CLIENT_SUITE_HARNESS_ORDINALS_H_
#define SRC_TESTS_FIDL_DYNSUITE_CLIENT_SUITE_HARNESS_ORDINALS_H_

#include <zircon/fidl.h>

#include <cstdint>

namespace client_suite {

/*
To find all ordinals:

grep 'constexpr.*k.*Target_.*Ordinal' \
out/default/fidling/gen/src/tests/fidl/dynsuite/client_suite/fidl/fidl.clientsuite/cpp/fidl/fidl.clientsuite/cpp/wire_messaging.cc
\ | sed -E 's/.* k(\w+Target_.*)_Ordinal = (.*);$/const uint64_t kOrdinal_\1 = \2;/'

We define ordinals here even though they're accessible in the C++ bindings
because the harness is designed to never use the bindings-under-test.
*/

// Target ordinals
const uint64_t kOrdinal_ClosedTarget_TwoWayNoPayload = 8823160117673072416lu;
const uint64_t kOrdinal_ClosedTarget_TwoWayStructPayload = 4197391242806919613lu;
const uint64_t kOrdinal_ClosedTarget_TwoWayTablePayload = 6034278263774013116lu;
const uint64_t kOrdinal_ClosedTarget_TwoWayUnionPayload = 2303581613023334284lu;
const uint64_t kOrdinal_ClosedTarget_TwoWayStructPayloadErr = 7112457167486827531lu;
const uint64_t kOrdinal_ClosedTarget_TwoWayStructRequest = 5762209808438257078lu;
const uint64_t kOrdinal_ClosedTarget_TwoWayTableRequest = 4411466441619884495lu;
const uint64_t kOrdinal_ClosedTarget_TwoWayUnionRequest = 8852700443574767223lu;
const uint64_t kOrdinal_ClosedTarget_OneWayNoRequest = 880285710761691653lu;
const uint64_t kOrdinal_ClosedTarget_OneWayStructRequest = 2745184144532233426lu;
const uint64_t kOrdinal_ClosedTarget_OneWayTableRequest = 1894745725882679327lu;
const uint64_t kOrdinal_ClosedTarget_OneWayUnionRequest = 1453322211393998309lu;
const uint64_t kOrdinal_ClosedTarget_OnEventNoPayload = 5685716274827179686lu;
const uint64_t kOrdinal_ClosedTarget_OnEventStructPayload = 5253669518495638118lu;
const uint64_t kOrdinal_ClosedTarget_OnEventTablePayload = 8251567748156745542lu;
const uint64_t kOrdinal_ClosedTarget_OnEventUnionPayload = 7621842152112557728lu;
const uint64_t kOrdinal_OpenTarget_StrictOneWay = 7904024199254697828lu;
const uint64_t kOrdinal_OpenTarget_FlexibleOneWay = 1119507890521778684lu;
const uint64_t kOrdinal_OpenTarget_StrictTwoWay = 995098632483095874lu;
const uint64_t kOrdinal_OpenTarget_StrictTwoWayFields = 8775166568163566307lu;
const uint64_t kOrdinal_OpenTarget_StrictTwoWayErr = 6063427118545698440lu;
const uint64_t kOrdinal_OpenTarget_StrictTwoWayFieldsErr = 9059738677182539537lu;
const uint64_t kOrdinal_OpenTarget_FlexibleTwoWay = 7373850992214410631lu;
const uint64_t kOrdinal_OpenTarget_FlexibleTwoWayFields = 4087393006943873764lu;
const uint64_t kOrdinal_OpenTarget_FlexibleTwoWayErr = 7932048879253591162lu;
const uint64_t kOrdinal_OpenTarget_FlexibleTwoWayFieldsErr = 1029425766479692362lu;
const uint64_t kOrdinal_OpenTarget_StrictEvent = 3110049402570307488lu;
const uint64_t kOrdinal_OpenTarget_FlexibleEvent = 5824395118547404952lu;

// A fake ordinal that's unknown to the server
const uint64_t kOrdinalFakeUnknownMethod = 0x10ff10ff10ff10fflu;

}  // namespace client_suite

#endif  // SRC_TESTS_FIDL_DYNSUITE_CLIENT_SUITE_HARNESS_ORDINALS_H_
