// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_hyper;
use fuchsia_sync::Mutex;
use hyper;
use rustls::client::danger::{ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{
    CertificateDer, ServerName, SignatureVerificationAlgorithm, TrustAnchor, UnixTime,
};
use std::cell::RefCell;
use std::sync::Arc;
use thiserror::Error;

type DateTime = chrono::DateTime<chrono::FixedOffset>;
#[derive(Debug, PartialEq, Clone, Copy, Hash, Eq)]
pub enum HttpsDateErrorType {
    InvalidHostname,
    SchemeNotHttps,
    NoCertificatesPresented,
    NetworkError,
    NoDateInResponse,
    InvalidCertificateChain,
    CorruptLeafCertificate,
    DateFormatError,
}

/// An error encountered while retrieving time from a server.
#[derive(Error)]
pub struct HttpsDateError {
    /// The rough category of error.
    error_type: HttpsDateErrorType,
    /// The underlying error, if any, that triggered the error.
    source: Option<anyhow::Error>,
}

impl HttpsDateError {
    /// Create a new `HttpsDateError`.
    pub fn new(error_type: HttpsDateErrorType) -> Self {
        Self { error_type, source: None }
    }

    /// Add or replace the underlying source error.
    pub fn with_source(mut self, source: anyhow::Error) -> Self {
        self.source = Some(source);
        self
    }

    pub fn error_type(&self) -> HttpsDateErrorType {
        self.error_type
    }
}

/// An extension trait to simplify mapping general errors to `HttpsDateError`.
trait HttpsDateResultExt<T> {
    /// Map an error in a Result to HttpsDateError with the given error_type.
    fn httpsdate_err(self, error_type: HttpsDateErrorType) -> Result<T, HttpsDateError>;
}

impl<T, E> HttpsDateResultExt<T> for Result<T, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn httpsdate_err(self, error_type: HttpsDateErrorType) -> Result<T, HttpsDateError> {
        self.map_err(|e| HttpsDateError::new(error_type).with_source(anyhow::Error::new(e)))
    }
}

// Manual implementation provided to shorten output in logs.
impl std::fmt::Debug for HttpsDateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.source.as_ref() {
            None => self.error_type.fmt(formatter),
            Some(source) => {
                formatter.write_fmt(format_args!("{:?}: {:?}", self.error_type, source))
            }
        }
    }
}

impl std::fmt::Display for HttpsDateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}

// I'd love to drop RSA here, but google.com doesn't yet serve ECDSA
static ALLOWED_SIG_ALGS: &[&dyn SignatureVerificationAlgorithm] = &[
    webpki::ring::ECDSA_P256_SHA256,
    webpki::ring::ECDSA_P256_SHA384,
    webpki::ring::ECDSA_P384_SHA256,
    webpki::ring::ECDSA_P384_SHA384,
    webpki::ring::RSA_PKCS1_2048_8192_SHA256,
    webpki::ring::RSA_PKCS1_2048_8192_SHA384,
    webpki::ring::RSA_PKCS1_2048_8192_SHA512,
    webpki::ring::RSA_PKCS1_3072_8192_SHA384,
];

// Because we don't yet have a system time we need a custom verifier
// that records the handshake information needed to perform a deferred
// trust evaluation
#[derive(Default, Debug)]
struct RecordingVerifier {
    presented_certs: Mutex<RefCell<Vec<CertificateDer<'static>>>>,
}

impl RecordingVerifier {
    // Verify the certificate chain stored during the TLS handshake against the
    // given |time| and |trust_anchors| using standard TLS verification.
    pub fn verify(
        &self,
        dns_name: &ServerName<'_>,
        time: UnixTime,
        trust_anchors: &'static [TrustAnchor<'static>],
    ) -> Result<(), HttpsDateError> {
        let presented_certs = self.presented_certs.lock();
        let presented_certs = presented_certs.borrow();
        if presented_certs.len() == 0 {
            return Err(HttpsDateError::new(HttpsDateErrorType::NoCertificatesPresented));
        };

        let leaf = webpki::EndEntityCert::try_from(&presented_certs[0])
            .httpsdate_err(HttpsDateErrorType::CorruptLeafCertificate)?;

        leaf.verify_for_usage(
            ALLOWED_SIG_ALGS,
            trust_anchors,
            &presented_certs[1..],
            time,
            webpki::KeyUsage::server_auth(),
            None,
            None,
        )
        .httpsdate_err(HttpsDateErrorType::InvalidCertificateChain)?;

        leaf.verify_is_valid_for_subject_name(dns_name)
            .httpsdate_err(HttpsDateErrorType::InvalidCertificateChain)
    }
}

impl ServerCertVerifier for RecordingVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        // Don't attempt to verify trust, just store the necessary details
        // for deferred evaluation
        let mut presented_certs = Vec::with_capacity(1 + intermediates.len());
        presented_certs.push(end_entity.clone().into_owned());
        presented_certs.extend(intermediates.iter().cloned().map(|c| c.into_owned()));
        *self.presented_certs.lock().borrow_mut() = presented_certs;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// An HTTPS client that reports the contents of the response Date header.
pub struct NetworkTimeClient {
    /// The custom verifier used for certificate validation.
    verifier: Arc<RecordingVerifier>,
    /// The set of trust anchors used to verify a response.
    trust_anchors: &'static [TrustAnchor<'static>],
    /// The underlying client for making requests.
    client: fuchsia_hyper::HttpsClient,
}

impl NetworkTimeClient {
    /// Create a new `NetworkTimeClient` that uses the trust anchors provided through
    /// the 'root-ssl-certificates' component feature.
    pub fn new() -> Self {
        Self::new_with_trust_anchors(&webpki_roots_fuchsia::TLS_SERVER_ROOTS)
    }

    fn new_with_trust_anchors(trust_anchors: &'static [TrustAnchor<'static>]) -> Self {
        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(trust_anchors.iter().cloned());

        // Because we don't currently have any idea what the "true" time is
        // we need to use a non-standard verifier, `RecordingVerifier`, to allow
        // us to defer trust evaluation until after we've parsed the response.
        let verifier = Arc::new(RecordingVerifier::default());
        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        config
            .dangerous()
            .set_certificate_verifier(Arc::clone(&verifier) as Arc<dyn ServerCertVerifier>);

        let client = fuchsia_hyper::new_https_client_dangerous(config, Default::default());

        NetworkTimeClient { verifier, client, trust_anchors }
    }

    /// Makes a best effort to get network time via an HTTPS connection to
    /// `uri`.
    ///
    /// # Errors
    ///
    /// `get_network_time` will return errors for network failures and TLS failures.
    ///
    /// # Panics
    ///
    /// `httpdate` needs access to the `root-ssl-certificates` sandbox feature. If
    /// it is not available this API will panic.
    ///
    /// # Security
    ///
    /// Validation of the TLS connection is deferred until after the handshake
    /// and then performed with respect to the time provided by the remote host.
    /// We validate the TLS connection against the system rootstore and time the server
    /// reports. This does mean that the best we can guarantee is that the host
    /// certificates were valid at some point, but the server can always provide a date
    /// that falls into the validity period of the certificates they provide.
    pub async fn get_network_time(&mut self, uri: hyper::Uri) -> Result<DateTime, HttpsDateError> {
        match uri.scheme_str() {
            Some("https") => (),
            _ => return Err(HttpsDateError::new(HttpsDateErrorType::SchemeNotHttps)),
        }
        let dns_name = match uri.host() {
            Some(host) => ServerName::try_from(host)
                .map_err(|_| HttpsDateError::new(HttpsDateErrorType::InvalidHostname))?
                .to_owned(),
            None => return Err(HttpsDateError::new(HttpsDateErrorType::InvalidHostname)),
        };

        let response =
            self.client.get(uri.clone()).await.httpsdate_err(HttpsDateErrorType::NetworkError)?;

        // Ok, so now we pull the Date header out of the response.
        // Technically the Date header is the date of page creation, but it's the best
        // we can do in the absence of a defined "accurate time" request.
        //
        // This has been suggested as being wrapped by an X-HTTPSTIME header,
        // or .well-known/time, but neither of these proposals appear to
        // have gone anywhere.
        let date_header: String = match response.headers().get("date") {
            Some(date) => {
                date.to_str().httpsdate_err(HttpsDateErrorType::DateFormatError)?.to_string()
            }
            _ => return Err(HttpsDateError::new(HttpsDateErrorType::NoDateInResponse)),
        };

        // Per RFC7231 the date header is specified as RFC2822 with a UTC timezone.
        let response_time = DateTime::parse_from_rfc2822(&date_header)
            .httpsdate_err(HttpsDateErrorType::DateFormatError)?;
        if response_time.timezone().utc_minus_local() != 0 {
            return Err(HttpsDateError::new(HttpsDateErrorType::DateFormatError));
        }

        // Finally verify the the certificate chain against the response time
        let webpki_time = UnixTime::since_unix_epoch(std::time::Duration::from_secs(
            response_time.timestamp() as u64,
        ));
        self.verifier.verify(&dns_name, webpki_time, self.trust_anchors)?;
        Ok(response_time)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use base64::engine::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use fuchsia_async as fasync;
    use futures::future::ready;
    use futures::stream::StreamExt;
    use hyper::{Response, StatusCode};
    use std::convert::Infallible;
    use std::net::{Ipv6Addr, SocketAddr};
    use std::sync::LazyLock;

    static TEST_CERT_CHAIN: LazyLock<Vec<CertificateDer<'static>>> = LazyLock::new(|| {
        parse_pem(&include_str!("../certs/server.certchain"))
            .into_iter()
            .map(CertificateDer::from)
            .collect()
    });
    static TEST_PRIVATE_KEY: LazyLock<rustls::pki_types::PrivateKeyDer<'static>> =
        LazyLock::new(|| {
            rustls::pki_types::PrivateKeyDer::Pkcs1(
                parse_pem(&include_str!("../certs/server.rsa")).pop().unwrap().into(),
            )
        });
    static CERT_NOT_BEFORE: LazyLock<DateTime> = LazyLock::new(|| {
        DateTime::parse_from_rfc3339(include_str!("../certs/notbefore").trim()).unwrap()
    });
    static CERT_NOT_AFTER: LazyLock<DateTime> = LazyLock::new(|| {
        DateTime::parse_from_rfc3339(include_str!("../certs/notafter").trim()).unwrap()
    });
    static TEST_CERT_ROOT: LazyLock<CertificateDer<'static>> = LazyLock::new(|| {
        CertificateDer::from(parse_pem(&include_str!("../certs/ca.cert")).pop().unwrap())
    });
    static TEST_TRUST_ANCHORS: LazyLock<Vec<TrustAnchor<'static>>> = LazyLock::new(|| {
        vec![webpki::anchor_from_trusted_cert(&TEST_CERT_ROOT).unwrap().to_owned()]
    });

    /// Spawn an HTTPS server that signs responses with TEST_PRIVATE_KEY and always returns
    /// `served_time` in the Date header. Listens for requests on 'localhost:port', where port
    /// is the returned port number.
    fn serve_fake(served_time: DateTime) -> u16 {
        let addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), 0);
        let listener = fasync::net::TcpListener::bind(&addr).unwrap();
        let server_port = listener.local_addr().unwrap().port();

        // build a server configuration using a test CA and cert chain
        let tls_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(TEST_CERT_CHAIN.clone(), TEST_PRIVATE_KEY.clone_key())
            .unwrap();

        let tls_acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls_config));
        let served_time_arc = Arc::new(served_time);

        fasync::Task::spawn(async move {
            let mut listener = listener;
            loop {
                match listener.accept().await {
                    Ok((next_listener, conn, _)) => {
                        listener = next_listener;
                        let tls_acceptor = tls_acceptor.clone();
                        let time_arc = Arc::clone(&served_time_arc);
                        fasync::Task::spawn(async move {
                            if let Ok(tls_stream) =
                                tls_acceptor.accept(fuchsia_hyper::TcpStream { stream: conn }).await
                            {
                                let io = hyper_util::rt::TokioIo::new(tls_stream);
                                let service = hyper::service::service_fn(
                                    move |_req: hyper::Request<hyper::body::Incoming>| {
                                        let time = Arc::clone(&time_arc);
                                        ready(Ok::<_, Infallible>(
                                            Response::builder()
                                                .header("Date", time.to_rfc2822())
                                                .status(StatusCode::OK)
                                                .body(http_body_util::Full::new(
                                                    hyper::body::Bytes::from(""),
                                                ))
                                                .unwrap(),
                                        ))
                                    },
                                );
                                let _ = hyper_util::server::conn::auto::Builder::new(
                                    fuchsia_hyper::Executor,
                                )
                                .serve_connection(io, service)
                                .await;
                            }
                        })
                        .detach();
                    }
                    Err(_) => break,
                }
            }
        })
        .detach();

        server_port
    }

    /// Serve a fake server that crashes when receiving a request.
    fn serve_crash() -> u16 {
        let addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), 0);
        let listener = fasync::net::TcpListener::bind(&addr).unwrap();
        let server_port = listener.local_addr().unwrap().port();

        let connection_dropper =
            listener.accept_stream().for_each(|conn_result| ready(drop(conn_result)));

        fasync::Task::spawn(connection_dropper).detach();

        server_port
    }

    /// Simple pem parser that doesn't validate format.
    fn parse_pem(contents: &str) -> Vec<Vec<u8>> {
        // Blindly assume format is correct for our test
        let mut parsed = vec![];
        let mut current_encoded = vec![];
        for line in contents.split('\n') {
            if line.starts_with("-----BEGIN") {
                ()
            } else if line.starts_with("-----END") {
                let encoded = current_encoded.join("");
                current_encoded = vec![];
                parsed.push(BASE64_STANDARD.decode(&encoded).unwrap());
            } else {
                current_encoded.push(line.trim());
            }
        }
        parsed
    }

    #[fuchsia::test]
    async fn test_get_network_time() {
        let set_time = *CERT_NOT_BEFORE + chrono::Duration::days(1);
        let open_port = serve_fake(set_time.clone());

        let mut client = NetworkTimeClient::new_with_trust_anchors(&TEST_TRUST_ANCHORS);

        let url = format!("https://localhost:{}/", open_port).parse::<hyper::Uri>().unwrap();
        let date = client.get_network_time(url).await.unwrap();
        assert_eq!(date, set_time);
    }

    #[fuchsia::test]
    async fn test_network_err() {
        let open_port = serve_crash();

        let mut client = NetworkTimeClient::new_with_trust_anchors(&TEST_TRUST_ANCHORS);

        let url = format!("https://localhost:{}/", open_port).parse::<hyper::Uri>().unwrap();
        assert_eq!(
            client.get_network_time(url).await.unwrap_err().error_type(),
            HttpsDateErrorType::NetworkError
        );
    }

    #[fuchsia::test]
    async fn test_untrusted_cert() {
        let time = *CERT_NOT_BEFORE + chrono::Duration::days(1);
        let open_port = serve_fake(time);

        // The test cert vended by our server should be rejected if we verify against real server
        // roots.
        let mut client =
            NetworkTimeClient::new_with_trust_anchors(&webpki_roots_fuchsia::TLS_SERVER_ROOTS);

        let url = format!("https://localhost:{}/", open_port).parse::<hyper::Uri>().unwrap();
        assert_eq!(
            client.get_network_time(url).await.unwrap_err().error_type(),
            HttpsDateErrorType::InvalidCertificateChain
        );
    }

    #[fuchsia::test]
    async fn test_time_after_cert_expired() {
        let time = *CERT_NOT_AFTER + chrono::Duration::days(2);
        let open_port = serve_fake(time);

        let mut client = NetworkTimeClient::new_with_trust_anchors(&TEST_TRUST_ANCHORS);

        let url = format!("https://localhost:{}/", open_port).parse::<hyper::Uri>().unwrap();
        assert_eq!(
            client.get_network_time(url).await.unwrap_err().error_type(),
            HttpsDateErrorType::InvalidCertificateChain
        );
    }

    #[fuchsia::test]
    async fn test_http_rejected() {
        let mut client = NetworkTimeClient::new_with_trust_anchors(&TEST_TRUST_ANCHORS);
        let url = "http://localhost/".parse::<hyper::Uri>().unwrap();
        assert_eq!(
            client.get_network_time(url).await.unwrap_err().error_type(),
            HttpsDateErrorType::SchemeNotHttps
        );
    }

    #[fuchsia::test]
    async fn test_bad_timezone() {
        let set_time = (*CERT_NOT_BEFORE + chrono::Duration::days(1))
            .with_timezone(&chrono::FixedOffset::east_opt(1 * 60 * 60).unwrap());
        let open_port = serve_fake(set_time.clone());

        let mut client = NetworkTimeClient::new_with_trust_anchors(&TEST_TRUST_ANCHORS);

        let url = format!("https://localhost:{}/", open_port).parse::<hyper::Uri>().unwrap();
        assert_eq!(
            client.get_network_time(url).await.unwrap_err().error_type(),
            HttpsDateErrorType::DateFormatError
        );
    }
}
