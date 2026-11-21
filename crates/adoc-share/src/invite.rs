//! Invitations (design §5): one line the host copies and sends by a channel it trusts, naming
//! the address it listens on, its certificate's fingerprint and a single-use secret. The host
//! keeps only each secret's hash, until it is used or expires.

use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    str::FromStr,
    time::{Duration, Instant},
};

use adoc_collab::protocol::Secret;
use sha2::{Digest, Sha256};

use crate::{Fingerprint, ShareError};

const PREFIX: &str = "adocfoundry:join?";

/// What a guest needs to join: where, whom to expect there, and the secret to show.
#[derive(Clone, Eq, PartialEq)]
pub struct Invitation {
    pub address: SocketAddr,
    pub fingerprint: Fingerprint,
    pub secret: Secret,
}

impl fmt::Debug for Invitation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Invitation")
            .field("address", &self.address)
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for Invitation {
    /// `adocfoundry:join?v=1&host=<address>&port=<port>&fp=<fingerprint>&secret=<secret>`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{PREFIX}v=1&host={}&port={}&fp={}&secret={}",
            self.address.ip(),
            self.address.port(),
            self.fingerprint.to_hex(),
            self.secret.to_hex()
        )
    }
}

impl FromStr for Invitation {
    type Err = ShareError;

    /// Reads an invitation line, surrounding white space ignored. Every field must be there
    /// once; the host is an address, never a name to look up.
    fn from_str(text: &str) -> Result<Self, ShareError> {
        let invalid = || ShareError::Invitation;
        let query = text.trim().strip_prefix(PREFIX).ok_or_else(invalid)?;
        let (mut version, mut host, mut port, mut fingerprint, mut secret) =
            (None, None, None, None, None);
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').ok_or_else(invalid)?;
            let slot = match key {
                "v" => &mut version,
                "host" => &mut host,
                "port" => &mut port,
                "fp" => &mut fingerprint,
                "secret" => &mut secret,
                _ => return Err(invalid()),
            };
            if slot.replace(value).is_some() {
                return Err(invalid());
            }
        }
        if version != Some("1") {
            return Err(invalid());
        }
        let ip = IpAddr::from_str(host.ok_or_else(invalid)?).map_err(|_| invalid())?;
        let port = port
            .ok_or_else(invalid)?
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(invalid)?;
        Ok(Self {
            address: SocketAddr::new(ip, port),
            fingerprint: Fingerprint::from_hex(fingerprint.ok_or_else(invalid)?)
                .ok_or_else(invalid)?,
            secret: Secret::from_hex(secret.ok_or_else(invalid)?).ok_or_else(invalid)?,
        })
    }
}

/// A random 256-bit secret from the system's source.
pub(crate) fn random_secret() -> Result<Secret, ShareError> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|error| ShareError::Io(error.to_string()))?;
    Ok(Secret(bytes))
}

pub(crate) fn hash(secret: &Secret) -> [u8; 32] {
    Sha256::digest(secret.0).into()
}

/// The host's outstanding invitations, as hashes with their expiry.
#[derive(Default)]
pub(crate) struct Invitations(Vec<([u8; 32], Instant)>);

impl Invitations {
    /// A new secret valid for `lifetime` from `now`.
    pub(crate) fn issue(&mut self, now: Instant, lifetime: Duration) -> Result<Secret, ShareError> {
        let secret = random_secret()?;
        self.0.push((hash(&secret), now + lifetime));
        Ok(secret)
    }

    /// Whether `secret` is an outstanding invitation's, using it up if so.
    pub(crate) fn redeem(&mut self, secret: &Secret, now: Instant) -> bool {
        self.0.retain(|(_, expires)| *expires > now);
        let wanted = hash(secret);
        let before = self.0.len();
        self.0.retain(|(held, _)| *held != wanted);
        self.0.len() < before
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invitation() -> Invitation {
        Invitation {
            address: "192.168.1.20:4567".parse().unwrap(),
            fingerprint: Fingerprint([0xab; 32]),
            secret: Secret([0x01; 32]),
        }
    }

    #[test]
    fn an_invitation_is_one_line_read_back_whole() {
        let line = invitation().to_string();
        assert_eq!(
            line,
            format!(
                "adocfoundry:join?v=1&host=192.168.1.20&port=4567&fp={}&secret={}",
                "ab".repeat(32),
                "01".repeat(32)
            )
        );
        assert_eq!(
            format!("  {line}\n").parse::<Invitation>().unwrap(),
            invitation()
        );
        let v6 = Invitation {
            address: "[fe80::1]:9".parse().unwrap(),
            ..invitation()
        };
        assert_eq!(v6.to_string().parse::<Invitation>().unwrap(), v6);
    }

    #[test]
    fn anything_else_is_not_an_invitation() {
        let line = invitation().to_string();
        let fp = "ab".repeat(32);
        let secret = "01".repeat(32);
        for bad in [
            String::new(),
            line.replace("adocfoundry:", "https:"),
            line.replace("v=1", "v=2"),
            line.replace("host=192.168.1.20", "host=example.com"),
            line.replace("port=4567", "port=0"),
            line.replace("port=4567", "port=70000"),
            line.replace(&fp, &fp[..62]),
            line.replace(&secret, &"G".repeat(64)),
            format!("{line}&secret={secret}"),
            format!("{line}&extra=1"),
            format!("adocfoundry:join?v=1&host=1.2.3.4&port=1&fp={fp}"),
        ] {
            assert_eq!(
                bad.parse::<Invitation>(),
                Err(ShareError::Invitation),
                "{bad}"
            );
        }
    }

    #[test]
    fn an_invitation_is_used_once_and_only_in_time() {
        let mut invitations = Invitations::default();
        let now = Instant::now();
        let secret = invitations.issue(now, Duration::from_secs(60)).unwrap();
        assert!(!invitations.redeem(&Secret([0; 32]), now));
        assert!(invitations.redeem(&secret, now));
        assert!(!invitations.redeem(&secret, now));
        let late = invitations.issue(now, Duration::from_secs(60)).unwrap();
        assert!(!invitations.redeem(&late, now + Duration::from_secs(61)));
    }

    #[test]
    fn printing_an_invitation_hides_its_secret() {
        assert!(!format!("{:?}", invitation()).contains("0101"));
    }
}
