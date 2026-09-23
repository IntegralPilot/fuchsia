// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import (
	"bufio"
	"os"
	"path/filepath"
	"strings"
)

// ParseGoMod reads a go.mod file and returns a slice of synthetic Readme structs,
// one for each required module, mapped to its location in the vendor directory.
func ParseGoMod(path string) ([]*Readme, error) {
	file, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer file.Close()

	var readmes []*Readme
	scanner := bufio.NewScanner(file)

	// We are looking for lines like:
	//   github.com/google/go-cmp v0.5.9
	// inside or outside of require blocks.

	inRequireBlock := false
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" || strings.HasPrefix(line, "//") || strings.HasPrefix(line, "module ") || strings.HasPrefix(line, "go ") {
			continue
		}

		if line == "require (" {
			inRequireBlock = true
			continue
		}
		if line == ")" {
			inRequireBlock = false
			continue
		}

		// Extract module path
		parts := strings.Fields(line)
		if len(parts) == 0 {
			continue
		}

		modulePath := ""
		if inRequireBlock {
			modulePath = parts[0]
		} else if strings.HasPrefix(line, "require ") {
			if len(parts) > 1 {
				modulePath = parts[1]
			}
		}

		if modulePath != "" {
			// Create a synthetic Readme for this module.
			// The location is assumed to be vendor/<modulePath> relative to the go.mod file.
			readmes = append(readmes, &Readme{
				Name:     filepath.Base(modulePath),
				URL:      "https://" + modulePath,
				Location: filepath.Join("vendor", modulePath),
			})
		}
	}

	return readmes, scanner.Err()
}
