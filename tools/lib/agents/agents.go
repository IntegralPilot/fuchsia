// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

// Package agents provides utilities to detect if the current process is running
// within an automated AI/agent environment.
package agents

import (
	_ "embed"
	"os"
	"strings"
)

//go:embed agents.txt
var agentsTxt string

var bakedAgentVars []string

func init() {
	// Parse the embedded agents.txt content at startup.
	for _, line := range strings.Split(agentsTxt, "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		// Strip inline comments.
		parts := strings.SplitN(line, "#", 2)
		val := strings.TrimSpace(parts[0])
		if val != "" {
			bakedAgentVars = append(bakedAgentVars, val)
		}
	}
}

// envLookup functions return the value of the environment variable and whether it is set.
type envLookup func(string) (string, bool)

// IsAgentEnv checks if the current environment indicates an AI agent.
func IsAgentEnv() bool {
	return isAgentEnvWithLookup(os.LookupEnv)
}

// isAgentEnvWithLookup checks if the environment (via lookup) indicates an AI agent.
func isAgentEnvWithLookup(lookup envLookup) bool {
	for _, v := range bakedAgentVars {
		if val, ok := lookup(v); ok && val != "" {
			return true
		}
	}
	return false
}
