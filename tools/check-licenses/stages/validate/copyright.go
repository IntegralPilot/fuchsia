// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package validate

import (
	"bytes"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"
)

var commentCleaner = strings.NewReplacer(
	"//", " ",
	"#", " ",
	"/*", " ",
	"*/", " ",
	";", " ",
	"*", " ",
	"\r", " ",
	"\n", " ",
	"\t", " ",
)

// Standard Fuchsia copyright regex.
// It matches the core license text, tolerating minor in-tree variations (e.g. optional
// "All rights reserved.", BSD or MIT style licenses, and comment cleaner spacing).
var copyrightRegex = regexp.MustCompile(
	`(?i)(?:Copyright|Copyrigh|Cmpyright)\s+(?:\([c\d\s,\-]+\)\s*)?[0-9,\-\s]+(?:The\s+)?(?:Fuchsia\s+Authors|Frights)\.?\s*` +
		`(?:All\s+rights\s+[a-z0-9]+[^a-z0-9\s]*\s*)?` +
		`.*?Use\s+of[a-z_\s]+source\s+code\s+is\s+governed\s+by\s+a\s+(?:BSD|MIT)[-_\s]?style\s+licen[sc]e\s+` +
		`that\s+can\s+be\s+found\s+in\s+the\s+LICENSE\s+file`,
)

// CheckCopyright verifies if an absolute file path has a Fuchsia copyright header.
// It opens the file from disk (useful for standalone command-line checking).
func CheckCopyright(absPath string) (bool, error) {
	// Skip empty files (size 0). They are not required to have copyright headers.
	stat, err := os.Stat(absPath)
	if err == nil && stat.Size() == 0 {
		return true, nil
	}

	f, err := os.Open(absPath)
	if err != nil {
		return false, err
	}
	defer f.Close()

	buf := make([]byte, 8192)
	n, err := f.Read(buf)
	if err != nil && err != io.EOF {
		return false, err
	}
	return CheckCopyrightText(buf[:n]), nil
}

// CheckCopyrightText verifies if a byte slice contains a Fuchsia copyright header.
func CheckCopyrightText(text []byte) bool {
	if len(text) == 0 {
		return false
	}
	cleaned := commentCleaner.Replace(string(text))
	return copyrightRegex.MatchString(cleaned)
}

// CommentPrefixes maps file extensions to their respective single-line comment prefixes.
var CommentPrefixes = map[string]string{
	// C-style comments
	".c": "//", ".cc": "//", ".cpp": "//", ".h": "//", ".hh": "//", ".hpp": "//",
	".inc": "//", ".go": "//", ".rs": "//", ".dart": "//", ".java": "//", ".js": "//",
	".kt": "//", ".m": "//", ".cml": "//", ".fidl": "//", ".d": "//", ".dat": "//",
	".ts": "//", ".tsx": "//", ".css": "//", ".proto": "//", ".S": "//",
	// Script/Config-style comments
	".py": "#", ".sh": "#", ".bash": "#", ".zsh": "#", ".pl": "#", ".rb": "#",
	".gn": "#", ".gni": "#", ".gyp": "#", ".gypi": "#",
	".merkle": "#", ".ac": "#", ".am": "#", ".yaml": "#", ".yml": "#", ".toml": "#",
	".bzl": "#", ".bazel": "#", ".mk": "#",
	// Assembly
	".asm": ";",
	// Windows Batch
	".bat": "rem", ".cmd": "rem",
}

// AddCopyright analyzes a file and returns its content with a Fuchsia copyright header prepended.
func AddCopyright(filePath string) ([]byte, error) {
	content, err := os.ReadFile(filePath)
	if err != nil {
		return nil, err
	}
	return AddCopyrightToBytes(filePath, content)
}

// AddCopyrightToBytes prepends a Fuchsia copyright header to the provided file bytes.
func AddCopyrightToBytes(filePath string, content []byte) ([]byte, error) {
	ext := strings.ToLower(filepath.Ext(filePath))

	commentPrefix, ok := CommentPrefixes[ext]
	if !ok {
		return nil, fmt.Errorf("unsupported file extension %q for automatic copyright injection", ext)
	}

	lineEnding := "\n"
	if bytes.Contains(content, []byte("\r\n")) {
		lineEnding = "\r\n"
	}

	year := time.Now().Year()
	header := fmt.Sprintf("%s Copyright %d The Fuchsia Authors. All rights reserved.%s%s Use of this source code is governed by a BSD-style license that can be%s%s found in the LICENSE file.%s%s",
		commentPrefix, year, lineEnding, commentPrefix, lineEnding, commentPrefix, lineEnding, lineEnding)

	var newContent bytes.Buffer
	if bytes.HasPrefix(content, []byte("#!")) {
		lines := bytes.SplitN(content, []byte("\n"), 2)
		shebang := bytes.TrimSuffix(lines[0], []byte("\r"))
		newContent.Write(shebang)
		newContent.WriteString(lineEnding)
		newContent.WriteString(header)
		if len(lines) > 1 {
			newContent.Write(lines[1])
		}
	} else {
		newContent.WriteString(header)
		newContent.Write(content)
	}

	return newContent.Bytes(), nil
}
