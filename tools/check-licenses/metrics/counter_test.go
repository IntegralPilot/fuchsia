// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package metrics

import (
	"sync"
	"testing"
)

func TestCounter_Basic(t *testing.T) {
	c := RegisterCounter("test_basic", "test", "label1", "label2")
	_ = c.Inc("val1", "val2")

	if count, _ := c.GetCount("val1", "val2"); count != 1 {
		t.Errorf("expected count 1, got %d", count)
	}
}

func TestCounter_MultipleDistinctLabels(t *testing.T) {
	c := RegisterCounter("test_multiple", "test", "l1", "l2")
	_ = c.Inc("a", "b")
	_ = c.Inc("x", "y")

	if count, _ := c.GetCount("a", "b"); count != 1 {
		t.Errorf("expected 1, got %d", count)
	}
	if count, _ := c.GetCount("x", "y"); count != 1 {
		t.Errorf("expected 1, got %d", count)
	}
}

func TestCounter_ErrorOnMismatchInc(t *testing.T) {
	c := RegisterCounter("test_error_inc", "test", "l1", "l2")

	if err := c.Inc("only_one_label"); err == nil {
		t.Errorf("expected error on mismatched labels")
	}
}

func TestCounter_ErrorOnMismatchGet(t *testing.T) {
	c := RegisterCounter("test_error_get", "test", "l1", "l2")

	if _, err := c.GetCount("only_one_label"); err == nil {
		t.Errorf("expected error on mismatched labels")
	}
}

func TestCounter_Concurrency(t *testing.T) {
	c := RegisterCounter("test_concurrency", "test", "label1")
	var wg sync.WaitGroup

	for i := 0; i < 100; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			_ = c.Inc("val")
		}()
	}

	wg.Wait()

	if count, _ := c.GetCount("val"); count != 100 {
		t.Errorf("expected 100, got %d", count)
	}
}
