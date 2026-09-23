// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package metrics

import (
	"sync"
	"testing"
	"time"
)

func TestTimer_BasicTracking(t *testing.T) {
	timer := RegisterTimer("test_timer_basic", "test")

	func() {
		defer timer.Track()()
		time.Sleep(10 * time.Millisecond)
	}()

	timer.mu.RLock()
	defer timer.mu.RUnlock()

	if timer.CallCount != 1 {
		t.Errorf("expected 1 call count, got %d", timer.CallCount)
	}
	if timer.TotalDuration < 10*time.Millisecond {
		t.Errorf("expected > 10ms, got %v", timer.TotalDuration)
	}
}

func TestTimer_MaxDuration(t *testing.T) {
	timer := RegisterTimer("test_timer_max", "test")

	func() {
		defer timer.Track()()
		time.Sleep(5 * time.Millisecond)
	}()

	func() {
		defer timer.Track()()
		time.Sleep(50 * time.Millisecond)
	}()

	timer.mu.RLock()
	defer timer.mu.RUnlock()

	if timer.CallCount != 2 {
		t.Errorf("expected 2 call count, got %d", timer.CallCount)
	}
	if timer.MaxDuration < 50*time.Millisecond {
		t.Errorf("expected >= 50ms for MaxDuration, got %v", timer.MaxDuration)
	}
}

func TestTimer_Concurrency(t *testing.T) {
	timer := RegisterTimer("test_timer_concurrency", "test")
	var wg sync.WaitGroup

	for i := 0; i < 100; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			defer timer.Track()()
			time.Sleep(1 * time.Millisecond)
		}()
	}

	wg.Wait()

	timer.mu.RLock()
	defer timer.mu.RUnlock()

	if timer.CallCount != 100 {
		t.Errorf("expected 100, got %d", timer.CallCount)
	}
	if timer.TotalDuration == 0 {
		t.Errorf("expected non-zero total duration")
	}
}

func TestTimer_ActiveDuration(t *testing.T) {
	timer := RegisterTimer("test_timer_active", "test")

	stop := timer.Track()
	time.Sleep(15 * time.Millisecond)

	activeDuration := timer.GetTotalDuration()
	if activeDuration < 10*time.Millisecond {
		t.Errorf("expected active duration >= 10ms, got %v", activeDuration)
	}

	stop()

	finalDuration := timer.GetTotalDuration()
	if finalDuration < 15*time.Millisecond {
		t.Errorf("expected final duration >= 15ms, got %v", finalDuration)
	}
}
