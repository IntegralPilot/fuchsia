// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[derive(Copy, Clone, Eq, PartialEq, Default, Debug)]
pub struct TaskTimeStats {
    pub user_time: zx::MonotonicDuration,
    pub system_time: zx::MonotonicDuration,
}

impl std::ops::Add for TaskTimeStats {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            user_time: self.user_time + other.user_time,
            system_time: self.system_time + other.system_time,
        }
    }
}

impl std::ops::AddAssign for TaskTimeStats {
    fn add_assign(&mut self, other: Self) {
        self.user_time += other.user_time;
        self.system_time += other.system_time;
    }
}
