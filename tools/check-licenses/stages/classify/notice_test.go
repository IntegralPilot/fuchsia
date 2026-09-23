// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package classify

import (
	"reflect"
	"testing"
)

func TestParseAndroid(t *testing.T) {
	content := []byte(`
===========================================================
The notices is included for the library: lib1
===========================================================
Some MIT License Text for lib1.
===========================================================
Not a notice block.
===========================================================
The notices is included for the library: lib2
===========================================================
Some Apache License Text for lib2.
===========================================================
`)
	chunks := parseAndroid(content)
	if len(chunks) != 2 {
		t.Fatalf("Expected 2 chunks, got %d", len(chunks))
	}
	if string(chunks[0]) != "Some MIT License Text for lib1." {
		t.Errorf("Unexpected chunk 0: %q", string(chunks[0]))
	}
	if string(chunks[1]) != "Some Apache License Text for lib2." {
		t.Errorf("Unexpected chunk 1: %q", string(chunks[1]))
	}
}

func TestParseChromium(t *testing.T) {
	content := []byte(`
Chromium Root License Text.
--------------------
lib1
--------------------
MIT License Text for lib1.
--------------------
lib2
--------------------
Apache License Text for lib2.
--------------------
`)
	chunks := parseChromium(content)
	if len(chunks) != 3 {
		t.Fatalf("Expected 3 chunks, got %d", len(chunks))
	}
	if string(chunks[0]) != "Chromium Root License Text." {
		t.Errorf("Unexpected chunk 0: %q", string(chunks[0]))
	}
	if string(chunks[1]) != "MIT License Text for lib1." {
		t.Errorf("Unexpected chunk 1: %q", string(chunks[1]))
	}
	if string(chunks[2]) != "Apache License Text for lib2." {
		t.Errorf("Unexpected chunk 2: %q", string(chunks[2]))
	}
}

func TestParseFlutter(t *testing.T) {
	content := []byte(`
====================================================================================================
LIBRARY: lib1
----------------------------------------------------------------------------------------------------
MIT License Text for lib1.
END OF TERMS AND CONDITIONS
Some extra stuff to ignore.
====================================================================================================
LIBRARY: lib2
----------------------------------------------------------------------------------------------------
Apache License Text for lib2.
`)
	chunks := parseFlutter(content)
	if len(chunks) != 2 {
		t.Fatalf("Expected 2 chunks, got %d", len(chunks))
	}
	if string(chunks[0]) != "MIT License Text for lib1." {
		t.Errorf("Unexpected chunk 0: %q", string(chunks[0]))
	}
	// Note: The second block does not have END OF TERMS AND CONDITIONS, but the code gracefully handles it
	// because SplitN will return 1 part.
	if string(chunks[1]) != "Apache License Text for lib2." {
		t.Errorf("Unexpected chunk 1: %q", string(chunks[1]))
	}
}

func TestParseGoogle(t *testing.T) {
	content := []byte(`
=================
lib1
MIT License Text for lib1.
=================
lib2

Apache License Text for lib2.
`)
	chunks := parseGoogle(content)
	if len(chunks) != 2 {
		t.Fatalf("Expected 2 chunks, got %d", len(chunks))
	}
	if string(chunks[0]) != "MIT License Text for lib1." {
		t.Errorf("Unexpected chunk 0: %q", string(chunks[0]))
	}
	if string(chunks[1]) != "Apache License Text for lib2." {
		t.Errorf("Unexpected chunk 1: %q", string(chunks[1]))
	}
}

func TestParseOneDelimiter(t *testing.T) {
	content := []byte(`
--------------------------------------------------------------------------------
lib1

MIT License Text for lib1.
--------------------------------------------------------------------------------
Root License Text without library name.
`)
	chunks := parseOneDelimiter(content)
	if len(chunks) != 2 {
		t.Fatalf("Expected 2 chunks, got %d", len(chunks))
	}
	if string(chunks[0]) != "MIT License Text for lib1." {
		t.Errorf("Unexpected chunk 0: %q", string(chunks[0]))
	}
	if string(chunks[1]) != "Root License Text without library name." {
		t.Errorf("Unexpected chunk 1: %q", string(chunks[1]))
	}
}

func TestDeduplicateChunks(t *testing.T) {
	chunks := [][]byte{
		[]byte("A"),
		[]byte("B"),
		[]byte("A"),
		[]byte("C"),
		[]byte("B"),
	}
	unique := deduplicateChunks(chunks)

	expected := [][]byte{
		[]byte("A"),
		[]byte("B"),
		[]byte("C"),
	}
	if !reflect.DeepEqual(unique, expected) {
		t.Errorf("Expected %v, got %v", expected, unique)
	}
}
