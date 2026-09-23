// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/node_add_args.h>

namespace fdf {

namespace fcd = fuchsia_component_decl;

fcd::Offer MakeOffer(std::string_view service_name, std::string_view instance_name) {
  auto mapping = fcd::NameMapping{{
      .source_name = std::string(instance_name),
      .target_name = component::kDefaultInstance,
  }};

  auto includes = std::string(component::kDefaultInstance);

  auto service = fcd::OfferService{{
      .source_name = std::string(service_name),
      .target_name = std::string(service_name),
      .source_instance_filter = std::vector{std::move(includes)},
      .renamed_instances = std::vector{std::move(mapping)},
  }};

  return fcd::Offer::WithService(service);
}

fcd::wire::Offer MakeOffer(fidl::AnyArena& arena, std::string_view service_name,
                           std::string_view instance_name) {
  fidl::VectorView<fcd::wire::NameMapping> mappings(arena, 1);
  mappings[0].source_name = fidl::StringView(arena, instance_name);
  mappings[0].target_name = fidl::StringView(arena, component::kDefaultInstance);

  fidl::VectorView<fidl::StringView> includes(arena, 1);
  includes[0] = fidl::StringView(arena, component::kDefaultInstance);

  auto offer = fcd::wire::OfferService::Builder(arena)
                   .source_name(arena, service_name)
                   .target_name(arena, service_name)
                   .renamed_instances(mappings)
                   .source_instance_filter(includes);

  return fcd::wire::Offer::WithService(arena, offer.Build());
}

}  // namespace fdf
