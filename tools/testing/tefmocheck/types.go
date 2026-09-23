// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package tefmocheck

import (
	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/testing/runtests"
)

// TestingOutputs are the outputs of a testing Swarming task that are analyzed by a FailureModeCheck.
type TestingOutputs struct {
	TestSummary           *runtests.TestSummary
	SwarmingSummary       *SwarmingTaskSummary
	SerialLogs            [][]byte
	SwarmingOutput        []byte
	SwarmingOutputPerTest []TestLog
	Syslogs               [][]byte
}

type logType string

// It's not necessary for correctness, but it makes the output easier for users if these
// strings match the file names that the recipes use for these logs. See the link above
// each constant for the relevant recipes code.
const (
	// https://fuchsia.googlesource.com/infra/recipes/+/cac1bc9f13f9349228e917dfae26bb85b1a290a8/recipe_modules/testing_requests/api.py#32
	serialLogType logType = "serial_log.txt"
	// https://fuchsia.googlesource.com/infra/recipes/+/cac1bc9f13f9349228e917dfae26bb85b1a290a8/recipe_modules/testing/api.py#22
	swarmingOutputType logType = "infra_and_test_std_and_klog.txt"
	// https://fuchsia.googlesource.com/infra/recipes/+/cac1bc9f13f9349228e917dfae26bb85b1a290a8/recipe_modules/testing_requests/api.py#33
	syslogType logType = "syslog.txt"
)

type logBlock struct {
	startString string
	endString   string
}

// FailureModeCheck checks whether a failure mode appears.
type FailureModeCheck interface {
	// Check analyzes TestingOutputs and returns true if the failure mode was detected.
	Check(*TestingOutputs) bool
	// Name is the name of this check.
	Name() string
	// DebugText is human-readable text intended to help debug a check failure.
	DebugText() string
	// OutputFiles are paths associated with this check. Often empty.
	OutputFiles() []string
	// IsFlake is true if the check is associated with a flaked test and
	// this check should also be reported as a flake instead of a failure.
	IsFlake() bool
	// Tags are the tags to append to the test result.
	Tags() []build.TestTag
	// IsInfraFailure is true if the check is a result of an
	// infrastructure failure.
	IsInfraFailure() bool
	// IsExoneration is true if the check should exonerate the matching test.
	IsExoneration() bool
	// FailureReason is the human-readable error message.
	FailureReason() string
	// EmitSyntheticTestCase is true if the check should add a synthetic
	// test case to every failed test. If testName is set, the synthetic test case
	// will be added to the specified test instead of every failed test.
	EmitSyntheticTestCase() bool
	// TestName returns the name of the test this check is attributed to,
	// or an empty string if the check is global (not attributed to a specific test).
	// When combined with EmitSyntheticTestCase(), it'll cause the synthetic test case
	// to be added to the specified test instead of every failed test where possible.
	TestName() string
}

// baseCheck provides default implementations of the FailureModeCheck interface.
// All types that inherit from baseCheck should override the Check() and Name() functions.
type baseCheck struct {
}

func (c baseCheck) Check(*TestingOutputs) bool {
	return true
}

func (c baseCheck) Name() string {
	return ""
}

func (c baseCheck) DebugText() string {
	return ""
}

func (c baseCheck) OutputFiles() []string {
	return []string{}
}

func (c baseCheck) IsFlake() bool {
	return false
}

func (c baseCheck) Tags() []build.TestTag {
	return nil
}

func (c baseCheck) IsInfraFailure() bool {
	return false
}

func (c baseCheck) IsExoneration() bool {
	return false
}

func (c baseCheck) FailureReason() string {
	return ""
}

func (c baseCheck) EmitSyntheticTestCase() bool {
	return false
}

func (c baseCheck) TestName() string {
	return ""
}
