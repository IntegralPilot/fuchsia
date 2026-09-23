// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/*
Package ninjacommand provides utilities to parse Ninja commands.

In particular, it can compute the trace event category of a given command,
and will properly recognize Fuchsia-specific wrapper scripts in it and
ignore them to get the final result.
*/
package ninjacommand

import (
	"path"
	"regexp"
	"strings"

	"github.com/google/shlex"
)

var pythonRE = regexp.MustCompile(`^python(\d(\.\d+)?)?$`)

// extractPythonScript returns the first python script in input tokens.
func extractPythonScript(tokens []string) string {
	for _, t := range tokens {
		if c := baseCmd(t); strings.HasSuffix(c, ".py") {
			return c
		}
	}
	return ""
}

// pythonWrapperScripts is a list of known python wrappers used in our build
// system. They should be stripped in order to get the real command of a build
// action.
var pythonWrapperScripts = []string{
	"action_tracer.py",
	"cxx_link_remote_wrapper.py",
	"cxx_remote_wrapper.py",
	"output_cacher.py",
	"output_leak_scanner.py",
	"prebuilt_tool_remote_wrapper.py",
	"restat_cacher.py",
	"rustc_remote_wrapper.py",
}

func isWrapperPythonScript(script string) bool {
	for _, s := range pythonWrapperScripts {
		if strings.HasSuffix(script, s) {
			return true
		}
	}
	return false
}

// extractCommand extracts the actual script/binary being run from a full
// command. This function includes logic to handle special wrapper scripts used
// in Fuchsia's build.
//
// This function is best-effort, if nothing can be successfully extracted, an
// empty string is returned.
func extractCommand(cmd []string) string {
	for i, token := range cmd {
		c := baseCmd(token)
		if c == "env" {
			continue
		}

		// gn_run_binary.sh is a script to run an arbitrary binary produced by the
		// current build.
		//
		// First argument to gn_run_binary.sh is the bin directory of the toolchain,
		// skip it to take the second argument, which is the binary to run.
		if c == "gn_run_binary.sh" {
			if i+2 < len(cmd) {
				return baseCmd(cmd[i+2])
			}
			break
		}

		if c == "reclient_cxx.sh" {
			// reclient_cxx.sh wraps a normal cxx build command to enable reclient.
			// The actual command is after the --.
			for j := i + 1; j < len(cmd); j++ {
				if cmd[j] == "--" && j+1 < len(cmd) {
					return extractCommand(cmd[j+1:])
				}
			}
		}

		if pythonRE.MatchString(c) {
			if script := extractPythonScript(cmd); script != "" {
				if !isWrapperPythonScript(script) {
					return script
				}
				// The top level command we extracted is one of the wrapper python
				// scripts, the actual command is after --, so abandon everything before
				// that.
				for j := i + 1; j < len(cmd); j++ {
					if cmd[j] == "--" && j+1 < len(cmd) {
						return extractCommand(cmd[j+1:])
					}
				}
			}
			break
		}

		return c
	}
	return ""
}

func baseCmd(cmd string) string {
	return strings.Trim(path.Base(cmd), "()")
}

func ComputeCommandCategories(command string) string {
	tokens, err := shlex.Split(command)
	if err != nil {
		return "invalid command"
	}

	var commands [][]string
	start := 0
	for i, t := range tokens {
		if t == "||" || t == "&&" {
			commands = append(commands, tokens[start:i])
			start = i + 1
		}
	}
	if start < len(tokens) {
		commands = append(commands, tokens[start:])
	}

	var categories []string
	for _, command := range commands {
		if c := extractCommand(command); c != "" {
			categories = append(categories, c)
		}
	}
	if len(categories) == 0 {
		return "unknown"
	}
	return strings.Join(categories, ",")
}
