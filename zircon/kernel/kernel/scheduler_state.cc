// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <kernel/scheduler.h>
#include <kernel/scheduler_state.h>
#include <ktl/algorithm.h>

void SchedulerState::RecomputeEffectiveProfile() {
  effective_profile_.AssertDirty();

  EffectiveProfile& ep = effective_profile_;
  const BaseProfile& bp = base_profile_;
  const InheritedProfileValues& ipv = inherited_profile_values_;

  ipv.AssertConsistency();

  if (bp.IsDeadline()) {
    const SchedUtilization total_util = ipv.uncapped_utilization + bp.deadline.utilization;
    const SchedUtilization new_util = ktl::min(total_util, Scheduler::kThreadUtilizationMax);
    const SchedDuration new_deadline = ktl::min(ipv.min_deadline, bp.deadline.deadline_ns);
    const SchedDuration new_capacity = new_util * new_deadline;

    ep.SetDeadline({new_capacity, new_deadline, new_util}, bp.critical || ipv.critical_count > 0);
  } else if (ipv.uncapped_utilization > SchedUtilization{0}) {
    const SchedUtilization new_util =
        ktl::min(ipv.uncapped_utilization, Scheduler::kThreadUtilizationMax);
    const SchedDuration capacity = new_util * ipv.min_deadline;
    ep.SetDeadline({capacity, ipv.min_deadline, new_util}, ipv.critical_count > 0);
  } else {
    // Our thread is fair.  We simply end up inheriting the total weight of the
    // threads blocked behind us.
    ep.SetFair(bp.fair.weight + ipv.total_weight);
  }

  effective_profile_.Clean();
}
