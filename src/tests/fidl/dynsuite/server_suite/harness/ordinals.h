// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_TESTS_FIDL_DYNSUITE_SERVER_SUITE_HARNESS_ORDINALS_H_
#define SRC_TESTS_FIDL_DYNSUITE_SERVER_SUITE_HARNESS_ORDINALS_H_

#include <cstdint>

namespace server_suite {

/*
To find all ordinals:

grep 'constexpr.*k.*Target_.*Ordinal' \
out/default/fidling/gen/src/tests/fidl/dynsuite/server_suite/fidl/fidl.serversuite/cpp/fidl/fidl.serversuite/cpp/wire_messaging.cc\
| sed -E 's/.* k(\w+Target_.*)_Ordinal = (.*);$/const uint64_t kOrdinal_\1 = \2;/'

We define ordinals here even though they're accessible in the C++ bindings
because the harness is designed to never use the bindings-under-test.
*/

// Target ordinals
const uint64_t kOrdinal_ClosedTarget_OneWayNoPayload = 462698674125537694lu;
const uint64_t kOrdinal_ClosedTarget_TwoWayNoPayload = 6618634609655918175lu;
const uint64_t kOrdinal_ClosedTarget_TwoWayStructPayload = 3546419415198665872lu;
const uint64_t kOrdinal_ClosedTarget_TwoWayTablePayload = 7142567342575659946lu;
const uint64_t kOrdinal_ClosedTarget_TwoWayUnionPayload = 8633460217663942074lu;
const uint64_t kOrdinal_ClosedTarget_TwoWayResult = 806800322701855052lu;
const uint64_t kOrdinal_ClosedTarget_GetHandleRights = 1195943399487699944lu;
const uint64_t kOrdinal_ClosedTarget_GetSignalableEventRights = 475344252578913711lu;
const uint64_t kOrdinal_ClosedTarget_EchoAsTransferableSignalableEvent = 6829189580925709472lu;
const uint64_t kOrdinal_ClosedTarget_ByteVectorSize = 1174084469162245669lu;
const uint64_t kOrdinal_ClosedTarget_HandleVectorSize = 5483915628125979959lu;
const uint64_t kOrdinal_ClosedTarget_CreateNByteVector = 2219580753158511713lu;
const uint64_t kOrdinal_ClosedTarget_CreateNHandleVector = 2752855654734922045lu;
const uint64_t kOrdinal_OpenTarget_StrictEvent = 538454334407181957lu;
const uint64_t kOrdinal_OpenTarget_FlexibleEvent = 4889200613481231166lu;
const uint64_t kOrdinal_OpenTarget_StrictOneWay = 2656433164255935131lu;
const uint64_t kOrdinal_OpenTarget_FlexibleOneWay = 4763610705738353240lu;
const uint64_t kOrdinal_OpenTarget_StrictTwoWay = 8071027055008411395lu;
const uint64_t kOrdinal_OpenTarget_StrictTwoWayFields = 3163464055637704720lu;
const uint64_t kOrdinal_OpenTarget_StrictTwoWayErr = 7997291255991962412lu;
const uint64_t kOrdinal_OpenTarget_StrictTwoWayFieldsErr = 3502827294789008624lu;
const uint64_t kOrdinal_OpenTarget_FlexibleTwoWay = 1871583035380534385lu;
const uint64_t kOrdinal_OpenTarget_FlexibleTwoWayFields = 5173692443570239348lu;
const uint64_t kOrdinal_OpenTarget_FlexibleTwoWayErr = 372287587009602464lu;
const uint64_t kOrdinal_OpenTarget_FlexibleTwoWayFieldsErr = 1925250685993373878lu;

// Epitaph ordinal
const uint64_t kOrdinalEpitaph = 0xfffffffffffffffflu;

// A fake ordinal that's unknown to the server
const uint64_t kOrdinalFakeUnknownMethod = 0x10ff10ff10ff10fflu;

}  // namespace server_suite

#endif  // SRC_TESTS_FIDL_DYNSUITE_SERVER_SUITE_HARNESS_ORDINALS_H_
