// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};
use log::info;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use {fidl_fuchsia_power_cpu as fcpu, fuchsia_async as fasync};

fn handle_domain_controller_stream(
    mut stream: fcpu::DomainControllerRequestStream,
    domains: Rc<Vec<fcpu::DomainInfo>>,
    max_frequency_index_map: Rc<RefCell<HashMap<u64, u64>>>,
) {
    fasync::Task::local(async move {
        while let Ok(Some(req)) = stream.try_next().await {
            match req {
                fcpu::DomainControllerRequest::ListDomains { responder } => {
                    responder.send(domains.as_ref()).unwrap();
                }
                fcpu::DomainControllerRequest::GetMaxFrequency { domain_id, responder } => {
                    if !max_frequency_index_map.borrow().contains_key(&domain_id) {
                        responder.send(Err(fcpu::GetMaxFrequencyError::InvalidArguments)).unwrap();
                        continue;
                    }

                    responder
                        .send(Ok(*max_frequency_index_map.borrow().get(&domain_id).unwrap()))
                        .unwrap();
                }
                fcpu::DomainControllerRequest::SetMaxFrequency {
                    domain_id,
                    frequency_index,
                    responder,
                } => {
                    if !max_frequency_index_map.borrow().contains_key(&domain_id) {
                        responder.send(Err(fcpu::SetMaxFrequencyError::InvalidArguments)).unwrap();
                        continue;
                    }

                    let frequencies = domains[0].available_frequencies_hz.as_ref().unwrap();
                    if frequency_index < frequencies.len() as u64 {
                        max_frequency_index_map.borrow_mut().insert(domain_id, frequency_index);
                        responder.send(Ok(())).unwrap();
                    } else {
                        responder.send(Err(fcpu::SetMaxFrequencyError::InvalidArguments)).unwrap();
                    }
                }
                fcpu::DomainControllerRequest::ClearMaxFrequency { domain_id, responder } => {
                    if !max_frequency_index_map.borrow().contains_key(&domain_id) {
                        responder
                            .send(Err(fcpu::ClearMaxFrequencyError::InvalidArguments))
                            .unwrap();
                        continue;
                    }

                    max_frequency_index_map.borrow_mut().insert(domain_id, 0);
                    responder.send(Ok(())).unwrap();
                }
                _ => unreachable!(),
            }
        }
    })
    .detach();
}

#[fuchsia::main]
async fn main() {
    info!("Started fake-thermal-sensor-manager");

    // Create domains such that each field differs to allow a variety of test
    // cases for clients.
    let domains = Rc::new(vec![
        fcpu::DomainInfo {
            id: Some(0),
            core_ids: Some(vec![0, 1]),
            available_frequencies_hz: Some(vec![2024000000, 1512000000, 1256000000, 1128000000]),
            name: Some("test-cluster0".to_string()),
            ..Default::default()
        },
        fcpu::DomainInfo {
            id: Some(1),
            core_ids: Some(vec![2, 3, 4, 5]),
            available_frequencies_hz: Some(vec![1024000000, 512000000]),
            name: Some("test-cluster1".to_string()),
            ..Default::default()
        },
    ]);

    let max_frequency_index_map: HashMap<u64, u64> =
        domains.iter().filter_map(|d| d.id.map(|id| (id, 0))).collect();
    assert_eq!(domains.len(), max_frequency_index_map.len());

    let max_frequency_index_map = Rc::new(RefCell::new(max_frequency_index_map));

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(move |stream| {
        handle_domain_controller_stream(stream, domains.clone(), max_frequency_index_map.clone())
    });

    fs.take_and_serve_directory_handle().unwrap();
    fs.collect::<()>().await;
}
