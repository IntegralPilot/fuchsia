// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package constants

const (
	// FailedToReconnectMsg is logged attempting to reconnect to SSH after an error fails.
	FailedToReconnectMsg = "failed to reconnect over SSH"
	// FailedToRunSnapshotMsg is logged if testrunner fails to run snapshot over ssh.
	FailedToRunSnapshotMsg = "failed to run snapshot over ssh"
	// FailedToStartSerialTestMsg is logged if testrunner repeatedly fails to run a
	// test over serial and gives up.
	FailedToStartSerialTestMsg = "failed to start test over serial"

	// A directory that will be automatically archived on completion of a task.
	TestOutDirEnvKey = "FUCHSIA_TEST_OUTDIR"
	// A path that host tests can write structured test case results to.
	TestOutputSummaryPathEnvKey = "TEST_OUTPUT_SUMMARY_PATH"
	// FFX config values that should be shared between infra and tests.
	FFXConfigsEnvKey = "FFX_CONFIGS"
	// A factor to multiply test timeouts by. Used when it is expected that tests will run
	// slower on specific target types/environments.
	TestTimeoutScaleFactor = "TEST_TIMEOUT_SCALE_FACTOR"
	// Used to run health-checks with DMS.
	DMCPathEnvKey       = "DMC_PATH"
	DMSPortEnvKey       = "DMS_PORT"
	NUCServerPortEnvKey = "NUC_SERVER_PORT"
)
