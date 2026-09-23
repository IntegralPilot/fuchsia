// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package tefmocheck

import (
	"fmt"
	"path"
)

type taskCheck struct {
	baseCheck
	swarmingSummary *SwarmingTaskSummary
}

func (c *taskCheck) DebugText() string {
	return debugTextForSwarmingSummary(c.swarmingSummary)
}

// taskStateCheck checks if the swarming task is in State.
type taskStateCheck struct {
	taskCheck
	State string
	// InfraFailure is true if the check is related to infra.
	InfraFailure bool
}

func (c *taskStateCheck) Check(to *TestingOutputs) bool {
	c.swarmingSummary = to.SwarmingSummary
	return to.SwarmingSummary.Results.State == c.State
}

func (c *taskStateCheck) Name() string {
	return path.Join("task_state", c.State)
}

func (c *taskStateCheck) IsInfraFailure() bool {
	return c.InfraFailure
}

// taskFailureCheck checks if the swarming task failed.
type taskFailureCheck struct {
	taskCheck
}

func (c *taskFailureCheck) Check(to *TestingOutputs) bool {
	c.swarmingSummary = to.SwarmingSummary
	return to.SwarmingSummary.Results.State == "COMPLETED" && to.SwarmingSummary.Results.Failure
}

func (c *taskFailureCheck) Name() string {
	return "task_failure"
}

// taskInternalFailureCheck checks if the swarming task internally failed.
type taskInternalFailureCheck struct {
	taskCheck
}

func (c *taskInternalFailureCheck) Check(to *TestingOutputs) bool {
	c.swarmingSummary = to.SwarmingSummary
	return to.SwarmingSummary.Results.State == "COMPLETED" && to.SwarmingSummary.Results.InternalFailure
}

func (c *taskInternalFailureCheck) Name() string {
	return "task_internal_failure"
}

func debugTextForSwarmingSummary(swarmingSummary *SwarmingTaskSummary) string {
	ret := fmt.Sprintf("Swarming task state: %s.", swarmingSummary.Results.State)
	if swarmingSummary.Results.Failure {
		ret += "\nTask failure."
	}
	if swarmingSummary.Results.InternalFailure {
		ret += "\nTask internal failure."
	}
	return fmt.Sprintf(`%s
The task's log is in %s.
The task URL is %s.
The task ran on bot %s.`,
		ret, swarmingOutputType, swarmingSummary.TaskURL(), swarmingSummary.BotURL())
}

// TaskStateChecks contains checks to cover every possible state.
// A task can only be in one state, so their relative order doesn't matter.
var TaskStateChecks []FailureModeCheck = []FailureModeCheck{
	// Covers state == COMPLETED.
	&taskInternalFailureCheck{},
	&taskFailureCheck{},
	// All other states.
	&taskStateCheck{State: "BOT_DIED", InfraFailure: true},
	&taskStateCheck{State: "CANCELED"},
	&taskStateCheck{State: "CLIENT_ERROR"},
	&taskStateCheck{State: "EXPIRED"},
	&taskStateCheck{State: "INVALID"},
	&taskStateCheck{State: "KILLED"},
	&taskStateCheck{State: "NO_RESOURCE", InfraFailure: true},
	&taskStateCheck{State: "PENDING"},
	&taskStateCheck{State: "RUNNING"},
	&taskStateCheck{State: "TIMED_OUT"},
}
