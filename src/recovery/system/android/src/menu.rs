// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub const MAIN_MENU: &[MenuItem] = &[
    MenuItem::Reboot,
    MenuItem::RebootBootloader,
    MenuItem::Sideload,
    MenuItem::WipeData,
    MenuItem::PowerOff,
];

pub const WIPE_DATA_MENU: &[MenuItem] = &[MenuItem::WipeDataCancel, MenuItem::WipeDataConfirm];

pub const PROMPT_WIPE_DATA_MENU: &[MenuItem] = &[MenuItem::Reboot, MenuItem::WipeData];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    Reboot,
    RebootBootloader,
    Sideload,
    WipeData,
    PowerOff,
    WipeDataConfirm,
    WipeDataCancel,
}

impl MenuItem {
    pub fn title(&self) -> &str {
        match self {
            MenuItem::Reboot => "Reboot system now",
            MenuItem::RebootBootloader => "Reboot to bootloader",
            MenuItem::Sideload => "Apply update from ADB",
            MenuItem::WipeData => "Wipe data/factory reset",
            MenuItem::PowerOff => "Power off",
            MenuItem::WipeDataConfirm => "Factory data reset",
            MenuItem::WipeDataCancel => "Cancel",
        }
    }
}

pub struct Menu {
    items: &'static [MenuItem],
    current_index: usize,
    is_active: bool,
}

impl Menu {
    pub fn new(items: &'static [MenuItem]) -> Menu {
        Menu { items, current_index: 0, is_active: false }
    }

    pub fn items(&self) -> &[MenuItem] {
        &self.items
    }

    pub fn current_item(&self) -> &MenuItem {
        &self.items[self.current_index]
    }

    pub fn is_active(&self) -> bool {
        self.is_active
    }

    pub fn set_active(&mut self, is_active: bool) {
        self.is_active = is_active;
    }

    pub fn move_up(&mut self) {
        self.is_active = false;
        if self.current_index > 0 {
            self.current_index -= 1;
        } else {
            self.current_index = self.items.len() - 1;
        }
    }

    pub fn move_down(&mut self) {
        self.is_active = false;
        self.current_index = (self.current_index + 1) % self.items.len();
    }
}
