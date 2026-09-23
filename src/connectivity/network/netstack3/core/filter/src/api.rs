// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use net_types::ip::{Ipv4, Ipv6};
use netstack3_base::{ContextPair, Inspector};

use crate::{
    FilterBindingsTypes, FilterContext, FilterIpContext, Routines, ValidRoutines, ValidationError,
};

/// The filtering API.
pub struct FilterApi<C>(C);

impl<C> FilterApi<C> {
    /// Creates a new `FilterApi` from a context pair.
    pub fn new(ctx: C) -> Self {
        Self(ctx)
    }
}

impl<C> FilterApi<C>
where
    C: ContextPair,
    C::CoreContext: FilterContext<C::BindingsContext>
        + FilterIpContext<Ipv4, C::BindingsContext>
        + FilterIpContext<Ipv6, C::BindingsContext>,
    C::BindingsContext: FilterBindingsTypes,
{
    fn core_ctx(&mut self) -> &mut C::CoreContext {
        let Self(pair) = self;
        pair.core_ctx()
    }

    /// Sets the filtering state for the provided IP version.
    ///
    /// The provided state must not contain any cyclical routine graphs (formed by
    /// rules with jump actions). The behavior in this case is unspecified but could
    /// be a deadlock or a panic, for example.
    ///
    /// # Panics
    ///
    /// Panics if the provided state includes cyclic routine graphs.
    pub fn set_filter_state<RuleInfo: Clone>(
        &mut self,
        v4: Routines<Ipv4, C::BindingsContext, RuleInfo>,
        v6: Routines<Ipv6, C::BindingsContext, RuleInfo>,
    ) -> Result<(), ValidationError<RuleInfo>> {
        let (v4_installed, v4_uninstalled) = ValidRoutines::new(v4)?;
        let (v6_installed, v6_uninstalled) = ValidRoutines::new(v6)?;

        self.core_ctx().with_all_filter_state_mut(|v4, v6| {
            if v4_installed.get().nat.contains_rules() {
                v4.nat_installed.set();
            }
            if v6_installed.get().nat.contains_rules() {
                v6.nat_installed.set();
            }

            v4.installed_routines = v4_installed;
            v4.uninstalled_routines = v4_uninstalled;
            v6.installed_routines = v6_installed;
            v6.uninstalled_routines = v6_uninstalled;
        });

        Ok(())
    }

    /// Exports filtering state into `inspector`.
    pub fn inspect_state<N: Inspector>(&mut self, inspector: &mut N) {
        inspector.record_child("IPv4", |inspector| {
            FilterIpContext::<Ipv4, _>::with_filter_state(self.core_ctx(), |state| {
                inspector.delegate_inspectable(state);
            });
        });
        inspector.record_child("IPv6", |inspector| {
            FilterIpContext::<Ipv6, _>::with_filter_state(self.core_ctx(), |state| {
                inspector.delegate_inspectable(state);
            });
        });
    }
}
