//! The host's identity for one session (design §5): a key pair and self-signed certificate made
//! in memory when sharing starts, never written anywhere, gone when sharing stops. Guests know
//! it by its fingerprint, from the invitation, and people compare its safety code.

use std::fmt;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

use crate::ShareError;

/// The SHA-256 of a certificate's DER encoding.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Fingerprint(pub [u8; 32]);

impl Fingerprint {
    /// The fingerprint of `certificate`.
    #[must_use]
    pub fn of(certificate: &[u8]) -> Self {
        Self(Sha256::digest(certificate).into())
    }

    /// The fingerprint as 64 lowercase hexadecimal digits.
    #[must_use]
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// A fingerprint written as 64 hexadecimal digits.
    #[must_use]
    pub fn from_hex(text: &str) -> Option<Self> {
        adoc_collab::protocol::Secret::from_hex(text).map(|secret| Self(secret.0))
    }

    /// The code both sides show for comparing by voice or in person: the first 64 bits as
    /// four groups of five decimal digits.
    #[must_use]
    pub fn safety_code(&self) -> SafetyCode {
        let group = |at: usize| u16::from_be_bytes([self.0[at], self.0[at + 1]]);
        SafetyCode([group(0), group(2), group(4), group(6)])
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Fingerprint({})", self.to_hex())
    }
}

/// Four groups of five digits, shown as `01234 56789 01234 56789`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SafetyCode(pub [u16; 4]);

impl fmt::Display for SafetyCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d] = self.0;
        write!(formatter, "{a:05} {b:05} {c:05} {d:05}")
    }
}

/// A host's certificate and key, for one session.
pub struct HostIdentity {
    certificate: CertificateDer<'static>,
    key: Vec<u8>,
    fingerprint: Fingerprint,
}

impl fmt::Debug for HostIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostIdentity")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl HostIdentity {
    /// A new key pair (ECDSA P-256, from the system's random source) and a certificate for it.
    pub fn generate() -> Result<Self, ShareError> {
        let made = rcgen::generate_simple_self_signed(vec!["adocfoundry.invalid".to_owned()])
            .map_err(|error| ShareError::Tls(error.to_string()))?;
        let certificate = made.cert.der().clone();
        let fingerprint = Fingerprint::of(&certificate);
        Ok(Self {
            certificate,
            key: made.signing_key.serialize_der(),
            fingerprint,
        })
    }

    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    pub(crate) fn certificate(&self) -> CertificateDer<'static> {
        self.certificate.clone()
    }

    pub(crate) fn key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_safety_code_is_the_fingerprints_first_64_bits() {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&[0x00, 0x01, 0xff, 0xff, 0x30, 0x39, 0x00, 0x00]);
        assert_eq!(
            Fingerprint(bytes).safety_code().to_string(),
            "00001 65535 12345 00000"
        );
    }

    #[test]
    fn each_identity_is_new_and_known_by_its_certificate() {
        let one = HostIdentity::generate().unwrap();
        let two = HostIdentity::generate().unwrap();
        assert_ne!(one.fingerprint(), two.fingerprint());
        assert_eq!(Fingerprint::of(&one.certificate()), one.fingerprint());
        assert_eq!(
            Fingerprint::from_hex(&one.fingerprint().to_hex()),
            Some(one.fingerprint())
        );
    }
}
