// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/connectivity/network/mdns/service/services/service_instance_publisher_service_impl.h"

#include "src/connectivity/network/mdns/service/common/mdns_fidl_util.h"
#include "src/connectivity/network/mdns/service/common/mdns_names.h"
#include "src/connectivity/network/mdns/service/common/type_converters.h"
#include "src/connectivity/network/mdns/service/encoding/dns_formatting.h"

namespace mdns {

ServiceInstancePublisherServiceImpl::ServiceInstancePublisherServiceImpl(
    Mdns& mdns, fidl::InterfaceRequest<fuchsia::net::mdns::ServiceInstancePublisher> request,
    fit::closure deleter)
    : ServiceImplBase<fuchsia::net::mdns::ServiceInstancePublisher>(mdns, std::move(request),
                                                                    std::move(deleter)),
      default_media_(Media::kBoth),
      default_ip_versions_(IpVersions::kBoth) {}

ServiceInstancePublisherServiceImpl::ServiceInstancePublisherServiceImpl(
    Mdns& mdns, DnsName host_name, std::vector<inet::IpAddress> addresses, Media default_media,
    IpVersions default_ip_versions,
    fidl::InterfaceRequest<fuchsia::net::mdns::ServiceInstancePublisher> request,
    fit::closure deleter)
    : ServiceImplBase<fuchsia::net::mdns::ServiceInstancePublisher>(mdns, std::move(request),
                                                                    std::move(deleter)),
      host_name_(std::move(host_name)),
      addresses_(std::move(addresses)),
      default_media_(default_media),
      default_ip_versions_(default_ip_versions) {}

void ServiceInstancePublisherServiceImpl::PublishServiceInstance(
    std::string service, std::string instance,
    fuchsia::net::mdns::ServiceInstancePublicationOptions options,
    fidl::InterfaceHandle<fuchsia::net::mdns::ServiceInstancePublicationResponder>
        publication_responder,
    PublishServiceInstanceCallback callback) {
  FX_DCHECK(publication_responder);

  DnsName service_name(std::move(service));

  if (!MdnsNames::IsValidServiceName(service_name)) {
    FX_LOGS(ERROR) << "PublishServiceInstance called with invalid service name " << service_name
                   << ", closing connection.";
    Quit(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (!MdnsNames::IsValidInstanceName(instance)) {
    FX_LOGS(ERROR) << "PublishServiceInstance called with invalid instance name " << instance
                   << ", closing connection.";
    Quit(ZX_ERR_INVALID_ARGS);
    return;
  }

  Media media = options.has_media() ? fidl::To<Media>(options.media()) : default_media_;
  IpVersions ip_versions = options.has_ip_versions() ? fidl::To<IpVersions>(options.ip_versions())
                                                     : default_ip_versions_;
  bool perform_probe = options.has_perform_probe() ? options.perform_probe() : true;

  auto publisher = new ResponderPublisher(publication_responder.Bind(), callback.share());

  if (!mdns().PublishServiceInstance(host_name_, addresses_, service_name, instance, media,
                                     ip_versions, perform_probe, publisher)) {
    delete publisher;
    callback(fpromise::error(
        fuchsia::net::mdns::PublishServiceInstanceError::ALREADY_PUBLISHED_LOCALLY));
    return;
  }
}

////////////////////////////////////////////////////////////////////////////////
// ServiceInstancePublisherServiceImpl::ResponderPublisher implementation

ServiceInstancePublisherServiceImpl::ResponderPublisher::ResponderPublisher(
    fuchsia::net::mdns::ServiceInstancePublicationResponderPtr responder,
    PublishServiceInstanceCallback callback)
    : responder_(std::move(responder)), callback_(std::move(callback)) {
  FX_DCHECK(responder_);
  FX_DCHECK(callback_);

  // This handler ensures that |Quit| will be called when the responder channel is closed. We don't
  // unbind from this end or remove the handler, except in the destructor. |Quit| causes this
  // publisher to be deleted, so the lifetime of the publisher is effectively scoped to the lifetime
  // of the channel.
  responder_.set_error_handler([this](zx_status_t status) mutable { MaybeDelete(); });

  responder_.events().SetSubtypes = [this](std::vector<std::string> subtypes) {
    for (const auto& subtype : subtypes) {
      if (!MdnsNames::IsValidSubtypeName(subtype)) {
        FX_LOGS(ERROR) << "Invalid subtype " << subtype
                       << " passed in SetSubtypes event, closing connection.";
        responder_ = nullptr;
        Quit();
        return;
      }
    }

    SetSubtypes(std::move(subtypes));
  };

  responder_.events().Reannounce = [this]() { Reannounce(); };
}

ServiceInstancePublisherServiceImpl::ResponderPublisher::~ResponderPublisher() {
  responder_.set_error_handler(nullptr);

  if (responder_.is_bound()) {
    responder_.Unbind();
  }
}

void ServiceInstancePublisherServiceImpl::ResponderPublisher::Quit() { MaybeDelete(); }

void ServiceInstancePublisherServiceImpl::ResponderPublisher::ReportSuccess(bool success) {
  FX_DCHECK(callback_);

  if (success) {
    callback_(fpromise::ok());
  } else {
    callback_(fpromise::error(
        fuchsia::net::mdns::PublishServiceInstanceError::ALREADY_PUBLISHED_ON_SUBNET));
  }
}

void ServiceInstancePublisherServiceImpl::ResponderPublisher::GetPublication(
    PublicationCause publication_cause, const std::string& subtype,
    const std::vector<inet::SocketAddress>& source_addresses, GetPublicationCallback callback) {
  // We ignore queries where the subtype is not utf-8. The fidl definition represents subtypes
  // as strings, so we are limited to utf-8 subtypes.
  // TODO(532641376): Use byte vectors for strings in the FIDL definitions.
  if (!utfutils_is_valid_utf8(subtype.data(), subtype.size())) {
    callback(nullptr);
    return;
  }

  if (on_publication_calls_in_progress_ < kMaxOnPublicationCallsInProgress) {
    ++on_publication_calls_in_progress_;
    GetPublicationNow(publication_cause, subtype, source_addresses, std::move(callback));
  } else {
    // If |pending_publications_| contains many publications already, ignore this one to
    // prevent memory exhaustion. The limit is quite high, so we don't bother doing anything
    // clever like tossing old requests.
    if (pending_publications_.size() >= kMaxPublicationsInQueue) {
      callback(nullptr);
      return;
    }

    pending_publications_.emplace(publication_cause, subtype, source_addresses,
                                  std::move(callback));
  }
}

void ServiceInstancePublisherServiceImpl::ResponderPublisher::OnGetPublicationComplete() {
  --on_publication_calls_in_progress_;
  if (!pending_publications_.empty() &&
      on_publication_calls_in_progress_ < kMaxOnPublicationCallsInProgress) {
    ++on_publication_calls_in_progress_;
    auto entry = std::move(pending_publications_.front());
    pending_publications_.pop();
    GetPublicationNow(entry.publication_cause_, std::move(entry.subtype_),
                      std::move(entry.source_addresses_), std::move(entry.callback_));
  }
}

void ServiceInstancePublisherServiceImpl::ResponderPublisher::GetPublicationNow(
    PublicationCause publication_cause, std::string subtype,
    std::vector<inet::SocketAddress> source_addresses, GetPublicationCallback callback) {
  FX_DCHECK(subtype.empty() || MdnsNames::IsValidSubtypeName(subtype));

  // The error handler for |responder_| may be called synchronously in the OnPublication proxy call
  // below. To ensure |this| doesn't get deleted while this method is running, we defer deletion
  // until the method terminates.
  DeferDeletion();

  FX_DCHECK(responder_);
  responder_->OnPublication(
      fidl::To<fuchsia::net::mdns::ServiceInstancePublicationCause>(publication_cause),
      subtype.empty() ? fidl::StringPtr() : fidl::StringPtr(std::move(subtype)),
      fidl::To<std::vector<fuchsia::net::IpAddress>>(source_addresses),
      [this, callback = std::move(callback)](
          fuchsia::net::mdns::ServiceInstancePublicationResponder_OnPublication_Result result) {
        if (result.is_err()) {
          OnGetPublicationComplete();
          return;
        }

        auto converted =
            fidl::To<std::unique_ptr<Mdns::Publication>>(result.response().publication);
        if (!converted) {
          Quit();
          return;
        }

        callback(std::move(converted));
        OnGetPublicationComplete();
      });

  MaybeDelete();
}

void ServiceInstancePublisherServiceImpl::ResponderPublisher::DeferDeletion() {
  ++one_based_delete_counter_;
}

void ServiceInstancePublisherServiceImpl::ResponderPublisher::MaybeDelete() {
  if (--one_based_delete_counter_ != 0) {
    return;
  }

  responder_.set_error_handler(nullptr);
  if (responder_.is_bound()) {
    responder_.Unbind();
  }
  delete this;
}

}  // namespace mdns
