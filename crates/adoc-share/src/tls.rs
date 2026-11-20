//! TLS 1.3 only, with ring as the provider, named in every configuration. The host presents its
//! session certificate; a guest accepts exactly the certificate its invitation names, and still
//! verifies the handshake's signature with it, so a server holding the certificate but not its
//! key fails.

use std::sync::Arc;

use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{ring, verify_tls13_signature, CryptoProvider},
    pki_types::{CertificateDer, ServerName, UnixTime},
    CertificateError, ClientConfig, DigitallySignedStruct, ServerConfig, SignatureScheme,
};

use crate::{Fingerprint, HostIdentity, ShareError};

/// The name a guest asks for; the certificate is pinned, so it is never checked.
pub(crate) const SERVER_NAME: &str = "adocfoundry.invalid";

fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

/// The host's configuration: its certificate, no client certificates.
pub fn server_config(identity: &HostIdentity) -> Result<Arc<ServerConfig>, ShareError> {
    let config = ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| ShareError::Tls(error.to_string()))?
        .with_no_client_auth()
        .with_single_cert(vec![identity.certificate()], identity.key())
        .map_err(|error| ShareError::Tls(error.to_string()))?;
    Ok(Arc::new(config))
}

/// A guest's configuration: only the certificate with `fingerprint` is accepted.
pub fn client_config(fingerprint: Fingerprint) -> Result<Arc<ClientConfig>, ShareError> {
    let provider = provider();
    let config = ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| ShareError::Tls(error.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned {
            fingerprint,
            provider,
        }))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

pub(crate) fn server_name() -> ServerName<'static> {
    ServerName::try_from(SERVER_NAME).expect("a valid name")
}

#[derive(Debug)]
struct Pinned {
    fingerprint: Fingerprint,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if intermediates.is_empty() && Fingerprint::of(end_entity) == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 is not used".to_owned()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            certificate,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
