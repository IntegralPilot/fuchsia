// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package constants

const (
	FailedToStartTargetMsg         = "start target error"
	FailedToCopyImageMsg           = "failed to copy image from GCS"
	QEMUInvocationErrorMsg         = "QEMU invocation error"
	ReadConfigFileErrorMsg         = "could not open config file"
	FailedToResolveIPErrorMsg      = "could not resolve target IP address"
	PackageRepoSetupErrorMsg       = "failed to set up a package repository"
	SerialReadErrorMsg             = "error reading serial log line"
	CommandExceededTimeoutMsg      = "Command exceeded timeout"
	FailedToServeMsg               = "[package server] failed to serve"
	FailedToCaptureSyslogMsg       = "failed to capture syslog"
	FailedToDeriveSshConnectionMsg = "failed to derive $SSH_CONNECTION"
	BotanistFailedMsg              = "botanist failed"

	NodenameEnvKey             = "FUCHSIA_NODENAME"
	SSHKeyEnvKey               = "FUCHSIA_SSH_KEY"
	SerialSocketEnvKey         = "FUCHSIA_SERIAL_SOCKET"
	SSHControlMasterPathEnvKey = "FUCHSIA_SSH_CONTROL_MASTER"
	// needed by boot tests
	SerialLogEnvKey  = "FUCHSIA_SERIAL_LOG"
	ECCableEnvKey    = "EC_CABLE_PATH"
	DeviceAddrEnvKey = "FUCHSIA_DEVICE_ADDR"
	DeviceTypeEnvKey = "FUCHSIA_DEVICE_TYPE" // Not set by botanist directly, but part of the host-target interaction API
	IPv4AddrEnvKey   = "FUCHSIA_IPV4_ADDR"
	IPv6AddrEnvKey   = "FUCHSIA_IPV6_ADDR"
	HostToolsEnvKey  = "FUCHSIA_HOST_TOOLS" // Path to host tools dir
	PkgSrvPortKey    = "FUCHSIA_PACKAGE_SERVER_PORT"
	// LINT.IfChange
	TestbedConfigEnvKey = "FUCHSIA_TESTBED_CONFIG"
	// LINT.ThenChange(//src/testing/end_to_end/mobly_driver/api_infra.py)

	FFXPathEnvKey       = "FUCHSIA_FFX_PATH"
	FFXConfigPathEnvKey = "FUCHSIA_FFX_CONFIG_PATH"
	FFXSharedDataEnvKey = "FUCHSIA_FFX_SHARED_DATA"

	FFXMonitorPort        = "FUCHSIA_FFX_MONITOR_PORT"
	DefaultFFXMonitorPort = "11000"

	// These env vars cause botanist to only run an explicitly specified list
	// of tests, skipping all others, for example when doing retries of
	// failed tests in presubmit. If the allowlist is empty, all tests will
	// be run.
	//
	// `TEST_ALLOWLIST_LENGTH ` lists how many tests there are to run, and
	// `TEST_ALLOWLIST_INDEX_N` variables specify the test at each index in
	// the list of tests to run (starting from 0).
	TestAllowlistLengthEnvKey        = "TEST_ALLOWLIST_LENGTH"
	TestAllowlistIndexEnvKeyTemplate = "TEST_ALLOWLIST_INDEX_%d"
)
