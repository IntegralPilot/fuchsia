// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package constants

const (
	// General ffx error.
	CommandFailedMsg       = "FFX command failed"
	ClientChannelClosedMsg = "There was an internal error running tests: Fidl(ClientChannelClosed { status: Status(PEER_CLOSED)"
	// LINT.IfChange
	TimeoutReachingTargetMsg = "Timeout attempting to reach target"
	// LINT.ThenChange(//src/testing/end_to_end/honeydew/honeydew/transports/ffx.py)
	UnableToResolveAddressMsg = "unable to resolve address for target"
)
