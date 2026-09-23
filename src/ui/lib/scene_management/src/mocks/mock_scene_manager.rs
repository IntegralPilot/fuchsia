// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use async_trait::async_trait;
use fidl_fuchsia_ui_app as ui_app;
use fidl_fuchsia_ui_views as ui_views;
use fidl_fuchsia_ui_views::ViewRef;
use fuchsia_async as fasync;
use scene_management::{DisplayMetrics, InjectorViewportSubscriber, SceneManagerTrait};
use std::cell::{Cell, RefCell};

pub struct MockSceneManager {
    was_present_root_view_called: Cell<bool>,
    was_set_root_view_called: Cell<bool>,
    set_root_view_viewport_token: Cell<Option<ui_views::ViewportCreationToken>>,
    set_root_view_view_ref: Cell<Option<ViewRef>>,
    tasks: RefCell<Vec<fasync::Task<()>>>,
}

impl MockSceneManager {
    pub fn new() -> Self {
        MockSceneManager {
            was_present_root_view_called: Cell::new(false),
            was_set_root_view_called: Cell::new(false),
            set_root_view_viewport_token: Cell::new(None),
            set_root_view_view_ref: Cell::new(None),
            tasks: RefCell::new(vec![]),
        }
    }

    pub fn assert_present_root_view_called(&self) {
        assert!(self.was_present_root_view_called.get() == true);
        self.was_present_root_view_called.set(false);
    }

    pub fn get_set_root_view_called_args(
        &self,
    ) -> (ui_views::ViewportCreationToken, Option<ui_views::ViewRef>) {
        assert!(self.was_set_root_view_called.get() == true);
        (
            self.set_root_view_viewport_token
                .take()
                .expect("Expected to have a viewport_token from a previous call to set_root_view."),
            self.set_root_view_view_ref.take(),
        )
    }
}

#[async_trait(?Send)]
impl SceneManagerTrait for MockSceneManager {
    fn present_root_view(&self) {
        self.was_present_root_view_called.set(true);
    }

    async fn set_root_view(
        &self,
        viewport_creation_token: ui_views::ViewportCreationToken,
        view_ref: Option<ui_views::ViewRef>,
    ) -> Result<(), Error> {
        self.was_set_root_view_called.set(true);
        self.set_root_view_viewport_token.set(Some(viewport_creation_token));
        self.set_root_view_view_ref.set(view_ref);
        Ok(())
    }

    // Leave everything else unimplemented.

    async fn set_root_view_deprecated(
        &self,
        _view_provider: ui_app::ViewProviderProxy,
    ) -> Result<ui_views::ViewRef, Error> {
        unimplemented!()
    }

    fn set_cursor_position(&self, _position_physical_px: input_pipeline::Position) {
        unimplemented!()
    }

    fn set_cursor_visibility(&self, _visible: bool) {
        unimplemented!()
    }

    fn get_pointerinjection_view_refs(&self) -> (ui_views::ViewRef, ui_views::ViewRef) {
        unimplemented!()
    }

    fn get_pointerinjection_display_size(&self) -> input_pipeline::Size {
        unimplemented!()
    }

    fn get_pointerinjector_viewport_watcher_subscription(&self) -> InjectorViewportSubscriber {
        unimplemented!()
    }

    fn get_display_metrics(&self) -> &DisplayMetrics {
        unimplemented!()
    }

    fn manage_task(&self, task: fasync::Task<()>) {
        self.tasks.borrow_mut().push(task);
    }
}
