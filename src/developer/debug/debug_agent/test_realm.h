// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_TEST_REALM_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_TEST_REALM_H_

#include <fidl/fuchsia.component.decl/cpp/fidl.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.sys2/cpp/fidl.h>

#include <string>

#include "src/developer/debug/shared/status.h"

namespace debug_agent {

// Values returned by |GetTestRealmAndOffers|.
struct TestRealmAndOffers {
  // The client end of the specified realm.
  fidl::ClientEnd<fuchsia_component::Realm> realm;

  std::vector<fuchsia_component_decl::Offer> offers;

  // The test collection portion of the |realm_arg| argument to |GetTestRealmAndOffers|.
  std::string test_collection;
};

// Gets the test realm and related offers specified by |realm_arg|, which takes the form
// <realm moniker>:<test collection>.
fit::result<debug::Status, TestRealmAndOffers> GetTestRealmAndOffers(
    const std::string& realm_arg,
    fidl::SyncClient<fuchsia_sys2::LifecycleController> lifecycle_controller,
    fidl::SyncClient<fuchsia_sys2::RealmQuery> realm_query);

// Gets the test realm and related offers specified by |realm_arg|, which takes the form
// <realm moniker>:<test collection>. This overload connects to |LifecycleController| and
// |RealmQuery| and calls the overload above.
fit::result<debug::Status, TestRealmAndOffers> GetTestRealmAndOffers(const std::string& realm_arg);

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_TEST_REALM_H_
