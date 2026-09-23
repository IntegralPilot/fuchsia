// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_JOB_POLICY_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_JOB_POLICY_H_

#include <stdint.h>
#include <zircon/syscalls/policy.h>
#include <zircon/types.h>

#include <kernel/timer.h>
// JobPolicyCollection is a storage container of two 64-bit words representing
// the encoded bit fields of each policy.
struct JobPolicyCollection {
  uint64_t storage[2];

  bool operator==(const JobPolicyCollection& other) const = default;
};

class JobPolicy;
extern "C" {
void rust_job_policy_create_root_policy(JobPolicy* out);
zx_status_t rust_job_policy_add_basic_policy(JobPolicy* policy, uint32_t mode,
                                             const zx_policy_basic_v2_t* policy_input,
                                             size_t policy_count);
uint32_t rust_job_policy_query_basic_policy(const JobPolicy* policy, uint32_t condition);
uint32_t rust_job_policy_query_basic_policy_override(const JobPolicy* policy, uint32_t condition);
void rust_job_policy_set_timer_slack(JobPolicy* policy, const TimerSlack* slack);
void rust_job_policy_get_timer_slack(const JobPolicy* policy, TimerSlack* out_slack);
bool rust_job_policy_eq(const JobPolicy* lhs, const JobPolicy* rhs);
void rust_job_policy_increment_counter(uint32_t action, uint32_t condition);
}

// JobPolicy is a value type that provides a space-efficient encoding of the policies defined in the
// policy.h public header.
//
// JobPolicy encodes two kinds of policy, basic and timer slack.
//
// Basic policy is logically an array of zx_policy_basic elements. For example:
//
//   zx_policy_basic policy[] = {
//      { ZX_POL_BAD_HANDLE, ZX_POL_ACTION_KILL },
//      { ZX_POL_NEW_CHANNEL, ZX_POL_ACTION_ALLOW },
//      { ZX_POL_NEW_FIFO, ZX_POL_ACTION_ALLOW_EXCEPTION },
//      { ZX_POL_VMAR_WX, ZX_POL_ACTION_KILL }}
//
// Timer slack policy defines the type and minimum amount of slack that will be applied to timer
// and deadline events.
class JobPolicy {
 public:
  JobPolicy() = delete;
  JobPolicy(const JobPolicy& parent) = default;
  JobPolicy& operator=(const JobPolicy& other) = default;
  static JobPolicy CreateRootPolicy();

  // Merge array |policy| of length |count| into this object.
  //
  // |mode| controls what happens when the policies in |policy| and this object intersect. |mode|
  // must be one of:
  //
  // ZX_JOB_POL_RELATIVE - Conflicting policies are ignored and will not cause the call to fail.
  //
  // ZX_JOB_POL_ABSOLUTE - If any of the policies in |policy| conflict with those in this object,
  //   the call will fail with an error and this object will not be modified.
  //
  zx_status_t AddBasicPolicy(uint32_t mode, const zx_policy_basic_v2_t* policy, size_t count);

  // Returns the action (e.g. ZX_POL_ACTION_ALLOW) for the specified |condition|.
  //
  // This method asserts if |policy| is invalid, and returns ZX_POL_ACTION_DENY for all other
  // failure modes.
  uint32_t QueryBasicPolicy(uint32_t condition) const;

  // Returns if the action for the specified condition can be overriden, so it returns
  // ZX_POL_OVERRIDE_ALLOW or ZX_POL_OVERRIDE_DENY.
  uint32_t QueryBasicPolicyOverride(uint32_t condition) const;

  // Sets the timer slack policy.
  //
  // |slack.amount| must be >= 0.
  void SetTimerSlack(TimerSlack slack);

  // Returns the timer slack policy.
  TimerSlack GetTimerSlack() const;

  bool operator==(const JobPolicy& rhs) const;
  bool operator!=(const JobPolicy& rhs) const;

  // Increment the kcounter for the given |action| and |condition|.
  //
  // action must be < ZX_POL_ACTION_MAX and condition must be < ZX_POL_MAX.
  //
  // For example: IncrementCounter(ZX_POL_ACTION_KILL, ZX_POL_NEW_CHANNEL);
  static void IncrementCounter(uint32_t action, uint32_t condition);

 private:
  struct UninitializedTag {};
  explicit JobPolicy(UninitializedTag) : slack_(TimerSlack::none()) {}

  // Remember, JobPolicy is a value type so think carefully before increasing its size.
  //
  // Const instances of JobPolicy must be immutable to ensure thread-safety.
  [[maybe_unused]] JobPolicyCollection collection_;
  [[maybe_unused]] TimerSlack slack_{TimerSlack::none()};
};

static_assert(sizeof(JobPolicyCollection) == 16);
static_assert(alignof(JobPolicyCollection) == 8);
static_assert(sizeof(JobPolicy) == 32);
static_assert(alignof(JobPolicy) == 8);

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_JOB_POLICY_H_
