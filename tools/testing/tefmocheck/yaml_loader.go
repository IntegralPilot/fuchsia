// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package tefmocheck

import (
	"fmt"
	"os"
	"slices"
	"strings"

	"gopkg.in/yaml.v2"
)

// yamlRoot represents the root structure of the YAML configuration file.
// Example:
//
//	failure_mode_checks:
//	  - kind: "string_in_log"
//	    string: "Exceeded safe temperature range"
//	    log_types:
//	      - syslogType
//	      - serialLogType
type yamlRoot struct {
	FailureChecks []checkEnvelope `yaml:"failure_mode_checks"`
}

// checkEnvelope represents the polymorphic envelope for unmarshaling different FailureModeChecks.
type checkEnvelope struct {
	Checks []FailureModeCheck
}

// All structs representing YAML configs must implement this interface.
type checkConfig interface {
	toChecks() ([]FailureModeCheck, error)
}

// LoadChecksFromFile reads a YAML file path and parses it into FailureModeChecks.
func LoadChecksFromFile(path string) ([]FailureModeCheck, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	return LoadChecksFromYAML(data)
}

// LoadChecksFromYAML parses a YAML byte slice and returns a slice of FailureModeChecks.
func LoadChecksFromYAML(data []byte) ([]FailureModeCheck, error) {
	var root yamlRoot
	if err := yaml.Unmarshal(data, &root); err != nil {
		return nil, fmt.Errorf("failed to unmarshal YAML: %w", err)
	}

	var checks []FailureModeCheck
	for _, env := range root.FailureChecks {
		checks = append(checks, env.Checks...)
	}

	return checks, nil
}

// UnmarshalYAML implements custom two-pass polymorphic unmarshaling.
func (env *checkEnvelope) UnmarshalYAML(unmarshal func(interface{}) error) error {
	// Pass 1: Extract the "kind" discriminator field
	var kindHelper struct {
		Kind string `yaml:"kind"`
	}
	if err := unmarshal(&kindHelper); err != nil {
		return err
	}

	kind := kindHelper.Kind

	// Pass 2: Unmarshal strictly into the type-specific struct
	// New FailureModeChecks that want to be configured via YAML should add a case to this switch.
	var cfg checkConfig
	switch kind {
	case "string_in_log":
		cfg = &stringInLogCheckConfig{}

	default:
		return fmt.Errorf("unknown check kind %q", kind)
	}

	if err := unmarshal(cfg); err != nil {
		return fmt.Errorf("failed to unmarshal check config for kind: %s: %v", kind, err)
	}

	checks, err := cfg.toChecks()
	if err != nil {
		return err
	}
	env.Checks = checks

	return nil
}

// Internal map used for friendlier error messages.
var logTypeMap = map[string]logType{
	"serialLogType":      serialLogType,
	"swarmingOutputType": swarmingOutputType,
	"syslogType":         syslogType,
}

func parseLogType(t string) (logType, error) {
	if lt, ok := logTypeMap[t]; ok {
		return lt, nil
	}
	// Collect keys for a more helpful error message.
	var keys []string
	for k := range logTypeMap {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	return "", fmt.Errorf("invalid log type %q (must be: %s)", t, strings.Join(keys, ", "))
}
