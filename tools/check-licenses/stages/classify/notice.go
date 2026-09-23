// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package classify

import (
	"bytes"
)

func deduplicateChunks(chunks [][]byte) [][]byte {
	seen := make(map[string]bool)
	var unique [][]byte
	for _, chunk := range chunks {
		s := string(chunk)
		if !seen[s] {
			seen[s] = true
			unique = append(unique, chunk)
		}
	}
	return unique
}

// parseAndroid extracts licenses from Android NOTICE files.
func parseAndroid(content []byte) [][]byte {
	var chunks [][]byte
	content = bytes.ReplaceAll(content, []byte("\r\n"), []byte("\n"))
	blocks := bytes.Split(content, []byte("==========================================================="))

	for i := 0; i < len(blocks)-1; i++ {
		block := bytes.TrimSpace(blocks[i])
		if bytes.Contains(block, []byte("The notices is included for the library:")) {
			text := bytes.TrimSpace(blocks[i+1])
			if len(text) > 0 {
				chunks = append(chunks, text)
			}
		}
	}
	return deduplicateChunks(chunks)
}

// parseChromium extracts licenses from Chromium NOTICE files.
func parseChromium(content []byte) [][]byte {
	var chunks [][]byte
	content = bytes.ReplaceAll(content, []byte("\r\n"), []byte("\n"))
	blocks := bytes.Split(content, []byte("--------------------"))

	if len(blocks) > 0 {
		text := bytes.TrimSpace(blocks[0])
		if len(text) > 0 {
			chunks = append(chunks, text)
		}
	}

	for i := 1; i < len(blocks)-1; i += 2 {
		text := bytes.TrimSpace(blocks[i+1])
		if len(text) > 0 {
			chunks = append(chunks, text)
		}
	}
	return deduplicateChunks(chunks)
}

// parseFlutter extracts licenses from Flutter NOTICE files.
func parseFlutter(content []byte) [][]byte {
	var chunks [][]byte
	content = bytes.ReplaceAll(content, []byte("\r\n"), []byte("\n"))
	blocks := bytes.Split(content, []byte("===================================================================================================="))

	for _, block := range blocks {
		block = bytes.TrimSpace(block)
		if len(block) == 0 {
			continue
		}

		parts := bytes.Split(block, []byte("----------------------------------------------------------------------------------------------------"))
		if len(parts) < 2 {
			continue
		}

		text := bytes.TrimSpace(parts[1])
		textParts := bytes.SplitN(text, []byte("END OF TERMS AND CONDITIONS"), 2)
		text = bytes.TrimSpace(textParts[0])

		if len(text) > 0 {
			chunks = append(chunks, text)
		}
	}
	return deduplicateChunks(chunks)
}

// parseGoogle extracts licenses from Google NOTICE files.
func parseGoogle(content []byte) [][]byte {
	var chunks [][]byte
	content = bytes.ReplaceAll(content, []byte("\r\n"), []byte("\n"))
	blocks := bytes.Split(content, []byte("================="))

	for _, block := range blocks {
		block = bytes.TrimSpace(block)
		if len(block) == 0 {
			continue
		}
		parts := bytes.SplitN(block, []byte("\n"), 2)
		if len(parts) > 1 {
			text := bytes.TrimSpace(parts[1])
			if len(text) > 0 {
				chunks = append(chunks, text)
			}
		}
	}
	return deduplicateChunks(chunks)
}

// parseOneDelimiter extracts licenses from generic single-delimiter NOTICE files.
func parseOneDelimiter(content []byte) [][]byte {
	var chunks [][]byte
	content = bytes.ReplaceAll(content, []byte("\r\n"), []byte("\n"))
	blocks := bytes.Split(content, []byte("--------------------------------------------------------------------------------"))

	for _, block := range blocks {
		block = bytes.TrimSpace(block)
		if len(block) == 0 {
			continue
		}

		parts := bytes.SplitN(block, []byte("\n\n"), 2)
		var text []byte
		if len(parts) > 1 {
			text = bytes.TrimSpace(parts[1])
		} else {
			text = parts[0]
		}
		if len(text) > 0 {
			chunks = append(chunks, text)
		}
	}
	return deduplicateChunks(chunks)
}
