// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/job_policy.h"

#include <assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls/policy.h>

#include <kernel/deadline.h>

// static
JobPolicy JobPolicy::CreateRootPolicy() {
  JobPolicy policy((UninitializedTag()));
  rust_job_policy_create_root_policy(&policy);
  return policy;
}

zx_status_t JobPolicy::AddBasicPolicy(uint32_t mode, const zx_policy_basic_v2_t* policy_input,
                                      size_t policy_count) {
  return rust_job_policy_add_basic_policy(this, mode, policy_input, policy_count);
}

uint32_t JobPolicy::QueryBasicPolicy(uint32_t condition) const {
  return rust_job_policy_query_basic_policy(this, condition);
}

uint32_t JobPolicy::QueryBasicPolicyOverride(uint32_t condition) const {
  return rust_job_policy_query_basic_policy_override(this, condition);
}

void JobPolicy::SetTimerSlack(TimerSlack slack) { rust_job_policy_set_timer_slack(this, &slack); }

TimerSlack JobPolicy::GetTimerSlack() const {
  TimerSlack slack = TimerSlack::none();
  rust_job_policy_get_timer_slack(this, &slack);
  return slack;
}

bool JobPolicy::operator==(const JobPolicy& rhs) const {
  if (this == &rhs) {
    return true;
  }
  return rust_job_policy_eq(this, &rhs);
}

bool JobPolicy::operator!=(const JobPolicy& rhs) const { return !operator==(rhs); }

void JobPolicy::IncrementCounter(uint32_t action, uint32_t condition) {
  rust_job_policy_increment_counter(action, condition);
}
