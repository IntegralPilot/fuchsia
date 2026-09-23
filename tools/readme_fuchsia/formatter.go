// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme_fuchsia

import (
	"reflect"
	"strings"
)

// Format takes a slice of Readme structs and serializes them into the
// canonical README.fuchsia text format, inserting the DEPENDENCY DIVIDER
// between sub-projects if multiple are present.
func Format(readmes []*Readme) string {
	var b strings.Builder

	for i, r := range readmes {
		if i > 0 {
			b.WriteString("\n" + dependencyDivider + "\n\n")
		}
		b.WriteString(formatSingle(r))
	}

	return b.String()
}

func formatSingle(r *Readme) string {
	var b strings.Builder
	readmeVal := reflect.ValueOf(r).Elem()
	t := readmeVal.Type()

	hasLicenses := len(r.Licenses) > 0 || len(r.LicenseFiles) > 0 || len(r.GeneratedNoticeFiles) > 0 || len(r.NonLicenseFiles) > 0
	printedLicensesNewline := false

	// We use reflection to iterate through the struct fields in the exact order
	// they are defined in types.go. This ensures canonical formatting order.
	for i := 0; i < t.NumField(); i++ {
		f := t.Field(i)

		if f.Name == "UnknownFields" {
			if len(r.UnknownFields) > 0 {
				b.WriteString("\n")
				for _, unknown := range r.UnknownFields {
					writeField(&b, unknown.Key, unknown.Value)
				}
			}
			continue
		}

		readmeTag := f.Tag.Get("readme")
		if readmeTag == "" || readmeTag == "-" {
			continue
		}

		key := strings.Split(readmeTag, ",")[0]
		isMultiline := f.Tag.Get("multiline") == "true"
		val := readmeVal.Field(i)

		// Formatting heuristics to match legacy behavior
		if hasLicenses && !printedLicensesNewline && (key == "License" || key == "License File" || key == "Generated Notice File" || key == "Non-License File") {
			b.WriteString("\n")
			printedLicensesNewline = true
		}

		if val.Kind() == reflect.Slice {
			if val.Len() == 0 {
				continue
			}
			var items []string
			for j := 0; j < val.Len(); j++ {
				items = append(items, val.Index(j).String())
			}

			// Legacy quirk: 'License' is printed as a comma-separated list on a single line,
			// whereas 'License File' prints each element on a new line (e.g., repeating the key).
			if key == "License" {
				writeField(&b, key, strings.Join(items, ", "))
			} else {
				for _, item := range items {
					writeField(&b, key, item)
				}
			}
		} else {
			strVal := val.String()
			if strVal != "" {
				if isMultiline {
					writeMultiLineField(&b, key, strVal)
				} else {
					writeField(&b, key, strVal)
				}
			}
		}
	}

	return strings.TrimRight(b.String(), "\n") + "\n"
}

func writeField(b *strings.Builder, key, value string) {
	if value != "" {
		b.WriteString(key + ": " + value + "\n")
	}
}

func writeMultiLineField(b *strings.Builder, key, value string) {
	if value != "" {
		b.WriteString("\n" + key + ":\n")

		value = strings.TrimSpace(value)
		lines := strings.Split(value, "\n")
		for _, line := range lines {
			if line == "" || strings.TrimSpace(line) == "" {
				b.WriteString("\n")
			} else {
				b.WriteString("  " + line + "\n")
			}
		}
	}
}
