// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package validate

import (
	"sort"
	"strings"
)

// Policy checks that are validated against the configuration.
const (
	PolicyUnrecognizedLicense = "AllLicenseTextsMustBeRecognized"
	PolicyUnrecognizedType    = "AllLicenseTypesMustBeRecognized"
	PolicyFuchsiaCopyright    = "AllFuchsiaAuthorSourceFilesMustHaveCopyrightHeaders"
	PolicyNoLicense           = "AllProjectsMustHaveALicense"
	PolicyNoReadme            = "AllProjectsMustHaveAReadme"
)

// Other check names used in the package.
const (
	CheckReadmeNeedsUpdate = "ReadmeFuchsiaNeedsUpdate"
	CheckPatternApproval   = "AllLicensePatternUsagesMustBeApproved"
)

type RuleMetadata struct {
	Bug         string
	Description string
	ConfigPath  string
}

type Config struct {
	PolicyExceptions    map[string]map[string]RuleMetadata
	AllowedLicenses     map[string]map[string]RuleMetadata
	CopyrightExtensions map[string]bool
	VirtualReadmeDir    func(projectPath string) string
}

// AddCopyrightExtension normalizes and adds a single file extension to CopyrightExtensions.
func (c *Config) AddCopyrightExtension(ext string) {
	if c == nil {
		return
	}
	if c.CopyrightExtensions == nil {
		c.CopyrightExtensions = make(map[string]bool)
	}
	if !strings.HasPrefix(ext, ".") {
		ext = "." + ext
	}
	c.CopyrightExtensions[ext] = true
}

// AddCopyrightExtensions normalizes and adds multiple file extensions to CopyrightExtensions.
func (c *Config) AddCopyrightExtensions(exts []string) {
	for _, ext := range exts {
		c.AddCopyrightExtension(ext)
	}
}

var validPolicyChecks = map[string]bool{
	PolicyUnrecognizedLicense: true,
	PolicyUnrecognizedType:    true,
	PolicyFuchsiaCopyright:    true,
	PolicyNoLicense:           true,
	PolicyNoReadme:            true,
}

// IsValidPolicy returns true if the given string is a valid policy check name.
func IsValidPolicy(name string) bool {
	return validPolicyChecks[name]
}

// ValidPolicies returns a sorted list of all valid policy check names.
func ValidPolicies() []string {
	policies := make([]string, 0, len(validPolicyChecks))
	for k := range validPolicyChecks {
		policies = append(policies, k)
	}
	sort.Strings(policies)
	return policies
}

// IsProjectScopePolicy returns true if the policy check applies to an entire project
// directory rather than an individual file.
func IsProjectScopePolicy(name string) bool {
	switch name {
	case PolicyNoLicense, PolicyNoReadme:
		return true
	default:
		return false
	}
}
