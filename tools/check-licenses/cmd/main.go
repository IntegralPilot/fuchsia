// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package main

import (
	"context"
	"flag"
	"os"
	"strings"

	"github.com/google/subcommands"
)

func main() {
	// Initialize the subcommand router
	commander := subcommands.NewCommander(flag.CommandLine, "check-licenses")

	// Register subcommands package's built-in commands
	commander.Register(commander.HelpCommand(), "")

	// Register our tool's commands
	commander.Register(&GenerateCommand{}, "")

	// Auditing & Remediation
	commander.Register(&ValidateCommand{}, "Auditing & Remediation")
	commander.Register(&FixCommand{}, "Auditing & Remediation")
	commander.Register(&ClassifyCommand{}, "Auditing & Remediation")
	commander.Register(&ProjectCommand{}, "Auditing & Remediation")
	commander.Register(&ReadmeCommand{}, "Auditing & Remediation")
	commander.Register(&CopyrightCommand{}, "Auditing & Remediation")

	// Policy Management
	commander.Register(&PolicyCommand{}, "Policy Management")
	commander.Register(&AllowlistCommand{}, "Policy Management")

	// Fallback routing for backward compatibility
	// If no valid subcommand is provided, we insert "validate" into os.Args.
	knownCommands := map[string]bool{
		"generate":  true,
		"validate":  true,
		"fix":       true,
		"readme":    true,
		"project":   true,
		"policy":    true,
		"allowlist": true,
		"copyright": true,
		"classify":  true,
		"help":      true,
	}

	insertValidate := true
	for _, arg := range os.Args[1:] {
		if !strings.HasPrefix(arg, "-") {
			if knownCommands[arg] {
				insertValidate = false
			}
			break
		}
	}

	if insertValidate {
		os.Args = append([]string{os.Args[0], "validate"}, os.Args[1:]...)
	}

	flag.Parse()
	ctx := context.Background()

	// Dispatch execution
	os.Exit(int(commander.Execute(ctx)))
}
