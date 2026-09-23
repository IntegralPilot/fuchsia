// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{anyhow, Context, Error};
use fidl_fuchsia_element as element;
use fuchsia_component::client::connect_to_protocol;
use log::info;

async fn propose_element(
    element_manager: element::ManagerProxy,
    config: element_launcher_config::Config,
) -> Result<(), Error> {
    let r: Result<Result<(), element::ManagerError>, fidl::Error> = element_manager
        .propose_element(
            element::Spec {
                component_url: Some(config.main_element_url),
                annotations: Some(vec![element::Annotation {
                    key: element::AnnotationKey {
                        namespace: "element_manager".to_string(),
                        value: "name".to_string(),
                    },
                    value: element::AnnotationValue::Text("main".to_string()),
                }]),
                ..Default::default()
            },
            None,
        )
        .await;
    match r {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(anyhow!("manager error: {e:?}")),
        Err(e) => Err(anyhow!("fidl error: {e}")),
    }
}

#[fuchsia::main(logging = true)]
async fn main() -> Result<(), Error> {
    info!("element_launcher is starting.");
    let config = element_launcher_config::Config::take_from_startup_handle();
    if config.main_element_url.is_empty() {
        return Err(anyhow!("element_launcher was started without a main_element_url; quitting."));
    }

    let element_manager = connect_to_protocol::<element::ManagerMarker>()
        .context("failed to connect to fuchsia.element.Manager")?;
    propose_element(element_manager, config).await.context("failed to propose element")
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[fuchsia::test]
    async fn test_propose_element() {
        let test_url = "test";
        let config = element_launcher_config::Config { main_element_url: test_url.to_string() };
        let (element_manager, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<element::ManagerMarker>();

        let stream_fut = async move {
            let request = stream.next().await.unwrap().unwrap();
            match request {
                element::ManagerRequest::ProposeElement { spec, responder, .. } => {
                    assert_eq!(spec.component_url, Some(test_url.to_string()));
                    responder.send(Ok(())).unwrap();
                }
                element::ManagerRequest::RemoveElement { .. } => {
                    panic!("RemoveElement was called");
                }
            };
        };
        let propose_fut = async {
            propose_element(element_manager, config).await.unwrap();
        };
        futures::join!(propose_fut, stream_fut);
    }
}
