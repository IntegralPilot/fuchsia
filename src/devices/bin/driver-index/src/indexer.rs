// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::composite_node_spec_manager::CompositeNodeSpecManager;
use crate::driver_loading_fuzzer::Session;
use crate::match_common::{node_to_device_property, node_to_device_property_no_autobind};
use crate::resolved_driver::{DriverPackageType, ResolvedDriver};
use bind::interpreter::decode_bind_rules::DecodedRules;
use fidl::endpoints::Proxy;
use fidl_fuchsia_component_resolution as fresolution;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_index as fdi;
use fidl_fuchsia_pkg_ext::BlobId;
use fuchsia_async as fasync;
use futures::StreamExt;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::str::FromStr;
use zx::Status;

#[derive(Default)]
pub enum BaseRepo {
    // We know that Base won't update so we can store these as resolved.
    Resolved(Vec<ResolvedDriver>),
    // If it's not resolved we store the clients waiting for it.
    #[default]
    NotResolved,
}

pub struct Indexer {
    pub boot_repo: RefCell<Vec<ResolvedDriver>>,

    // |base_repo| needs to be in a RefCell because it starts out NotResolved,
    // but will eventually resolve when base packages are available.
    pub base_repo: RefCell<BaseRepo>,

    // Contains the ephemeral drivers. This is wrapped in a RefCell since the
    // ephemeral drivers are added after the driver index server has started
    // through the FIDL API, fuchsia.driver.registrar.Register.
    pub ephemeral_drivers: RefCell<HashMap<cm_types::Url, ResolvedDriver>>,

    // Manages the specs. This is wrapped in a RefCell since the
    // specs are added after the driver index server has started.
    pub composite_node_spec_manager: RefCell<CompositeNodeSpecManager>,

    // Sends a notify message out through this when new drivers are available.
    driver_notifier: RefCell<Option<fdi::DriverNotifierProxy>>,

    // Used to determine if the indexer should return fallback drivers that match or
    // wait until based packaged drivers are indexed.
    delay_fallback_until_base_drivers_indexed: bool,
}

impl Indexer {
    pub fn new(
        boot_repo: Vec<ResolvedDriver>,
        base_repo: BaseRepo,
        delay_fallback_until_base_drivers_indexed: bool,
    ) -> Indexer {
        Indexer {
            boot_repo: RefCell::new(boot_repo),
            base_repo: RefCell::new(base_repo),
            ephemeral_drivers: RefCell::new(HashMap::new()),
            composite_node_spec_manager: RefCell::new(CompositeNodeSpecManager::new()),
            driver_notifier: RefCell::new(None),
            delay_fallback_until_base_drivers_indexed,
        }
    }

    pub fn start_driver_load(
        self: Rc<Self>,
        receiver: futures::channel::mpsc::UnboundedReceiver<Vec<ResolvedDriver>>,
        session: Session,
    ) {
        fasync::Task::local(async move {
            self.handle_add_boot_driver(receiver).await;
        })
        .detach();

        fasync::Task::local(session.run()).detach();
    }

    fn include_fallback_drivers(&self) -> bool {
        !self.delay_fallback_until_base_drivers_indexed
            || match *self.base_repo.borrow() {
                BaseRepo::Resolved(_) => true,
                _ => false,
            }
    }

    fn change_disabled_state_to(
        &self,
        driver_url: &str,
        package_hash: Option<String>,
        disabled_value: bool,
    ) -> u32 {
        let op = match disabled_value {
            true => "Disable".to_string(),
            false => "Enable".to_string(),
        };
        let mut count = 0;
        let requested_hash = package_hash.and_then(|hash| {
            let parsed_hash = fuchsia_hash::Hash::from_str(hash.as_str());
            match parsed_hash {
                Ok(h) => Some(BlobId::from(h)),
                Err(e) => {
                    log::warn!("Failed to parse package_hash in disable_driver: {:?}.", e);
                    log::warn!("Skipping hash comparisons.");
                    None
                }
            }
        });

        for driver in self.boot_repo.borrow_mut().iter_mut() {
            if driver.component_url.as_str() == driver_url {
                if requested_hash.is_some() && requested_hash != driver.package_hash {
                    log::warn!(
                        "{} found matching driver url, but skipping due to hash mismatch.",
                        op
                    );
                    continue;
                }

                if driver.disabled != disabled_value {
                    driver.disabled = disabled_value;
                    count += 1;
                }

                break;
            }
        }

        match self.base_repo.borrow_mut().deref_mut() {
            BaseRepo::Resolved(base_repo) => {
                for driver in base_repo.iter_mut() {
                    if driver.component_url.as_str() == driver_url {
                        if requested_hash.is_some() && requested_hash != driver.package_hash {
                            log::warn!(
                                "{} found matching driver url, but skipping due to hash mismatch.",
                                op
                            );
                            continue;
                        }

                        if driver.disabled != disabled_value {
                            driver.disabled = disabled_value;
                            count += 1;
                        }

                        break;
                    }
                }
            }
            BaseRepo::NotResolved => {}
        }

        for (_url, driver) in self.ephemeral_drivers.borrow_mut().iter_mut() {
            if driver.component_url.as_str() == driver_url {
                if requested_hash.is_some() && requested_hash != driver.package_hash {
                    log::warn!(
                        "{} found matching driver url, but skipping due to hash mismatch.",
                        op
                    );
                    continue;
                }

                if driver.disabled != disabled_value {
                    driver.disabled = disabled_value;
                    count += 1;
                }

                break;
            }
        }

        count
    }

    pub fn set_notifier(&self, notifier: fidl::endpoints::ClientEnd<fdi::DriverNotifierMarker>) {
        *self.driver_notifier.borrow_mut() = Some(notifier.into_proxy());
    }

    pub fn take_notifier(&self) -> Option<fidl::endpoints::ClientEnd<fdi::DriverNotifierMarker>> {
        let notifier = self.driver_notifier.borrow_mut().take();
        match notifier {
            Some(notifier) => match notifier.into_client_end() {
                Ok(client_end) => Some(client_end),
                Err(_) => None,
            },
            None => None,
        }
    }

    pub async fn handle_add_boot_driver(
        self: Rc<Self>,
        mut receiver: futures::channel::mpsc::UnboundedReceiver<Vec<ResolvedDriver>>,
    ) {
        while let Some(mut drivers) = receiver.next().await {
            log::info!("Loaded drivers into the driver index:");
            for driver in &drivers {
                log::info!("     {}", driver.component_url);
                let mut composite_node_spec_manager = self.composite_node_spec_manager.borrow_mut();
                composite_node_spec_manager.new_driver_available(driver);
            }
            self.boot_repo.borrow_mut().append(&mut drivers);
            self.report_driver_load();
        }
    }

    fn report_driver_load(&self) {
        match *self.driver_notifier.borrow() {
            Some(ref notifier) => {
                let result = notifier.new_driver_available();
                if let Err(e) = result {
                    log::warn!("Failed to send new_driver_available: {:?}", e);
                }
            }
            None => {}
        };
    }

    // Create a list of all drivers (except for disabled drivers) in the following priority:
    // 1. Non-fallback boot drivers
    // 2. Non-fallback base drivers
    // 3. Fallback boot drivers
    // 4. Fallback base drivers
    fn list_drivers(&self) -> Vec<ResolvedDriver> {
        let base_repo = self.base_repo.borrow();
        let base_repo_iter = match base_repo.deref() {
            BaseRepo::Resolved(drivers) => drivers.iter(),
            BaseRepo::NotResolved => [].iter(),
        };

        let boot_repo = self.boot_repo.borrow();
        let (boot_drivers, base_drivers) = if self.include_fallback_drivers() {
            (boot_repo.iter(), base_repo_iter.clone())
        } else {
            ([].iter(), [].iter())
        };
        let fallback_boot_drivers = boot_drivers.filter(|&driver| driver.fallback);
        let fallback_base_drivers = base_drivers.filter(|&driver| driver.fallback);

        let ephemeral = self.ephemeral_drivers.borrow();

        boot_repo
            .iter()
            .filter(|&driver| !driver.fallback)
            .chain(base_repo_iter.filter(|&driver| !driver.fallback))
            .chain(ephemeral.values())
            .chain(fallback_boot_drivers)
            .chain(fallback_base_drivers)
            .filter(|&driver| !driver.disabled)
            .map(|driver| driver.clone())
            .collect::<Vec<_>>()
    }

    pub fn load_base_repo(&self, base_drivers: Vec<ResolvedDriver>) {
        if let BaseRepo::NotResolved = self.base_repo.borrow_mut().deref_mut() {
            self.report_driver_load();
        }
        self.base_repo.replace(BaseRepo::Resolved(base_drivers));
    }

    pub fn match_driver(&self, args: fdi::MatchDriverArgs) -> fdi::DriverIndexMatchDriverResult {
        if args.properties.is_none() {
            log::error!("Failed to match driver: empty properties");
            return Err(Status::INVALID_ARGS.into_raw());
        }
        let properties = args.properties.unwrap();
        let properties = match args.driver_url_suffix {
            Some(_) => node_to_device_property_no_autobind(&properties)?,
            None => node_to_device_property(&properties)?,
        };

        // Prioritize specs to avoid match conflicts with composite drivers.
        let spec_match = self.composite_node_spec_manager.borrow().match_parent_specs(&properties);
        if let Some(spec) = spec_match {
            return Ok(spec);
        }

        let driver_list = self.list_drivers();

        let (mut fallback, mut non_fallback): (
            Vec<(bool, fdi::MatchDriverResult)>,
            Vec<(bool, fdi::MatchDriverResult)>,
        ) = driver_list
            .iter()
            .filter_map(|driver| {
                if let Ok(Some(matched)) = driver.matches(&properties) {
                    if let Some(url_suffix) = &args.driver_url_suffix {
                        if !driver.component_url.as_str().ends_with(url_suffix.as_str()) {
                            return None;
                        }
                    }

                    Some((driver.fallback, matched))
                } else {
                    None
                }
            })
            .partition(|(fallback, _)| *fallback);

        match (non_fallback.len(), fallback.len()) {
            (1, _) => Ok(non_fallback.pop().unwrap().1),
            (0, 1) => Ok(fallback.pop().unwrap().1),
            (0, 0) => Err(Status::NOT_FOUND.into_raw()),
            (0, _) => {
                log::error!(
                    "Failed to match driver: Encountered unsupported behavior: Zero non-fallback drivers and more than one fallback drivers were matched"
                );
                log::error!("Fallback drivers {:#?}", fallback);
                Err(Status::NOT_SUPPORTED.into_raw())
            }
            _ => {
                log::error!(
                    "Failed to match driver: Encountered unsupported behavior: Multiple non-fallback drivers were matched"
                );
                log::error!("Drivers {:#?}", non_fallback);
                Err(Status::NOT_SUPPORTED.into_raw())
            }
        }
    }

    pub fn match_pending_node(
        &self,
        dependencies: Vec<fdf::ParentSpec2>,
    ) -> Result<fdi::MatchPendingNodeResult, i32> {
        let parents = dependencies;

        let driver_list = self.list_drivers();
        let composite_drivers = driver_list
            .iter()
            .filter(|&driver| matches!(driver.bind_rules, DecodedRules::Composite(_)))
            .collect::<Vec<_>>();

        let mut fallback = vec![];
        let mut non_fallback = vec![];

        for driver in composite_drivers {
            let matched_composite_result =
                crate::composite_helper::match_composite_properties(driver, &parents);
            if let Ok(Some(matched_composite)) = matched_composite_result {
                if driver.fallback {
                    fallback.push(matched_composite);
                } else {
                    non_fallback.push(matched_composite);
                }
            }
        }

        let matched = match (non_fallback.len(), fallback.len()) {
            (1, _) => Ok(non_fallback.pop().unwrap()),
            (0, 1) => Ok(fallback.pop().unwrap()),
            (0, 0) => Err(Status::NOT_FOUND.into_raw()),
            (0, _) => {
                log::error!(
                    "Failed to match pending node: Encountered unsupported behavior: Zero non-fallback drivers and more than one fallback drivers were matched"
                );
                log::error!("Fallback drivers {:#?}", fallback);
                Err(Status::NOT_SUPPORTED.into_raw())
            }
            _ => {
                log::error!(
                    "Failed to match pending node: Encountered unsupported behavior: Multiple non-fallback drivers were matched"
                );
                log::error!("Drivers {:#?}", non_fallback);
                Err(Status::NOT_SUPPORTED.into_raw())
            }
        }?;

        Ok(fdi::MatchPendingNodeResult { driver: Some(matched), ..Default::default() })
    }

    pub fn add_composite_node_spec(&self, spec: fdf::CompositeNodeSpec) -> Result<(), i32> {
        let driver_list = self.list_drivers();
        let composite_drivers = driver_list
            .iter()
            .filter(|&driver| matches!(driver.bind_rules, DecodedRules::Composite(_)))
            .collect::<Vec<_>>();

        let mut composite_node_spec_manager = self.composite_node_spec_manager.borrow_mut();
        composite_node_spec_manager.add_composite_node_spec(spec, composite_drivers)
    }

    pub fn rebind_composite(
        &self,
        spec_name: String,
        driver_url_suffix: Option<String>,
    ) -> Result<(), i32> {
        let driver_list = self.list_drivers();
        let composite_drivers = driver_list
            .iter()
            .filter(|&driver| {
                if let Some(url_suffix) = &driver_url_suffix {
                    if !driver.component_url.as_str().ends_with(url_suffix.as_str()) {
                        return false;
                    }
                }
                matches!(driver.bind_rules, DecodedRules::Composite(_))
            })
            .collect::<Vec<_>>();
        self.composite_node_spec_manager.borrow_mut().rebind(spec_name, composite_drivers)
    }

    pub fn rebind_composites_with_driver(&self, driver: String) -> Result<(), i32> {
        let driver_list = self.list_drivers();
        let composite_drivers = driver_list
            .iter()
            .filter(|&driver| matches!(driver.bind_rules, DecodedRules::Composite(_)))
            .collect::<Vec<_>>();
        self.composite_node_spec_manager
            .borrow_mut()
            .rebind_composites_with_driver(driver, composite_drivers)
    }

    pub fn get_driver_info(&self, driver_filter: Vec<String>) -> Vec<fdf::DriverInfo> {
        let mut driver_info = Vec::new();

        for driver in self.boot_repo.borrow().iter() {
            if driver_filter.len() == 0
                || driver_filter.iter().any(|f| f == driver.component_url.as_str())
            {
                driver_info.push(driver.create_driver_info(true));
            }
        }

        let base_repo = self.base_repo.borrow();
        if let BaseRepo::Resolved(drivers) = &base_repo.deref() {
            for driver in drivers {
                if driver_filter.len() == 0
                    || driver_filter.iter().any(|f| f == driver.component_url.as_str())
                {
                    driver_info.push(driver.create_driver_info(true));
                }
            }
        }

        let ephemeral = self.ephemeral_drivers.borrow();
        for driver in ephemeral.values() {
            if driver_filter.len() == 0
                || driver_filter.iter().any(|f| f == driver.component_url.as_str())
            {
                driver_info.push(driver.create_driver_info(true));
            }
        }

        driver_info
    }

    pub async fn register_driver(
        &self,
        driver_url: String,
        resolver: &fresolution::ResolverProxy,
    ) -> Result<(), i32> {
        let component_url = cm_types::Url::new(&driver_url).map_err(|e| {
            log::error!("Couldn't parse driver url: {}: error: {}", driver_url, e);
            Status::ADDRESS_UNREACHABLE.into_raw()
        })?;
        for boot_driver in self.boot_repo.borrow().iter() {
            if boot_driver.component_url == component_url {
                log::warn!("Driver being registered already exists in boot list.");
                return Err(Status::ALREADY_EXISTS.into_raw());
            }
        }

        match self.base_repo.borrow().deref() {
            BaseRepo::Resolved(resolved_base_drivers) => {
                for base_driver in resolved_base_drivers {
                    if base_driver.component_url == component_url {
                        log::warn!("Driver being registered already exists in base list.");
                        return Err(Status::ALREADY_EXISTS.into_raw());
                    }
                }
            }
            _ => (),
        };

        let resolve =
            ResolvedDriver::resolve(component_url.clone(), &resolver, DriverPackageType::Universe)
                .await;

        if resolve.is_err() {
            return Err(resolve.err().unwrap().into_raw());
        }

        let resolved_driver = resolve.unwrap();

        let mut composite_node_spec_manager = self.composite_node_spec_manager.borrow_mut();
        composite_node_spec_manager.new_driver_available(&resolved_driver);

        let mut ephemeral_drivers = self.ephemeral_drivers.borrow_mut();
        let existing = ephemeral_drivers.insert(component_url.clone(), resolved_driver);

        if let Some(existing_driver) = existing {
            log::info!("Updating existing ephemeral driver {}.", existing_driver);
        } else {
            log::info!("Registered driver successfully: {}.", component_url);
        }

        Ok(())
    }

    pub fn disable_driver(
        &self,
        driver_url: String,
        package_hash: Option<String>,
    ) -> Result<(), i32> {
        let count = self.change_disabled_state_to(driver_url.as_str(), package_hash, true);

        if count == 0 {
            return Err(Status::NOT_FOUND.into_raw());
        }

        log::info!("Disabled {} driver(s).", count);

        Ok(())
    }

    pub fn enable_driver(
        &self,
        driver_url: String,
        package_hash: Option<String>,
    ) -> Result<(), i32> {
        let count = self.change_disabled_state_to(driver_url.as_str(), package_hash, false);

        if count == 0 {
            return Err(Status::NOT_FOUND.into_raw());
        }

        log::info!("Enabled {} driver(s).", count);

        Ok(())
    }
}
