// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod tests {
    use crate::routing::RoutingTestBuilderForAnalyzer;
    use ::routing_test_helpers::dictionary::CommonDictionaryTest;

    #[fuchsia::test]
    async fn use_protocol_from_dictionary() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_use_protocol_from_dictionary()
            .await
    }

    #[fuchsia::test]
    async fn use_protocol_from_dictionary_not_a_dictionary() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_use_protocol_from_dictionary_not_a_dictionary()
            .await
    }

    #[fuchsia::test]
    async fn use_protocol_from_dictionary_not_used() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_use_protocol_from_dictionary_not_used()
            .await
    }

    #[fuchsia::test]
    async fn use_protocol_from_dictionary_not_found() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_use_protocol_from_dictionary_not_found()
            .await
    }

    #[fuchsia::test]
    async fn use_directory_from_dictionary() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_use_directory_from_dictionary()
            .await
    }

    #[fuchsia::test]
    async fn expose_directory_from_dictionary() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_expose_directory_from_dictionary()
            .await
    }

    #[fuchsia::test]
    async fn use_protocol_from_nested_dictionary() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_use_protocol_from_nested_dictionary()
            .await
    }

    #[fuchsia::test]
    async fn offer_protocol_from_dictionary() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_offer_protocol_from_dictionary()
            .await
    }

    #[fuchsia::test]
    async fn offer_protocol_from_dictionary_not_found() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_offer_protocol_from_dictionary_not_found()
            .await
    }

    #[fuchsia::test]
    async fn offer_protocol_from_dictionary_to_dictionary() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_offer_protocol_from_dictionary_to_dictionary()
            .await
    }

    #[fuchsia::test]
    async fn offer_protocol_from_nested_dictionary() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_offer_protocol_from_nested_dictionary()
            .await
    }

    #[fuchsia::test]
    async fn expose_protocol_from_dictionary() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_expose_protocol_from_dictionary()
            .await
    }

    #[fuchsia::test]
    async fn expose_protocol_from_dictionary_not_found() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_expose_protocol_from_dictionary_not_found()
            .await
    }

    #[fuchsia::test]
    async fn expose_protocol_from_nested_dictionary() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_expose_protocol_from_nested_dictionary()
            .await
    }

    #[fuchsia::test]
    async fn dictionary_in_exposed_dir() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_dictionary_in_exposed_dir()
            .await
    }

    #[fuchsia::test]
    async fn offer_dictionary_to_dictionary() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_offer_dictionary_to_dictionary()
            .await
    }

    #[fuchsia::test]
    async fn use_from_dictionary_availability_invalid() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_use_from_dictionary_availability_invalid()
            .await
    }

    #[fuchsia::test]
    async fn offer_from_dictionary_availability_invalid() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_offer_from_dictionary_availability_invalid()
            .await
    }

    #[fuchsia::test]
    async fn expose_from_dictionary_availability_attenuated() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_expose_from_dictionary_availability_attenuated()
            .await
    }

    #[fuchsia::test]
    async fn expose_from_dictionary_availability_invalid() {
        CommonDictionaryTest::<RoutingTestBuilderForAnalyzer>::new()
            .test_expose_from_dictionary_availability_invalid()
            .await
    }
}
