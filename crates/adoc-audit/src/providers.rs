//! Optional providers: checks beyond the core audit, each needing something the core audit never
//! touches (an external tool, the network or the repository's history), so each is off until a
//! policy enables it.
//!
//! A provider's work never runs in this crate, which stays offline. Whoever runs it (the engine,
//! or a front end for the network) reports what happened to the [`crate::AuditBuilder`]
//! (`provider_ran`, or `provider_unavailable` with the reason), and its findings as findings of
//! its own registered rules. [`crate::AuditBuilder::finish`] gives every registered provider a
//! [`ProviderStatus`]: disabled, ran, or unavailable. An enabled provider that did not run, for
//! whatever reason, is unavailable and a `provider-unavailable` gap, so the report is partial
//! and never mistaken for a full one, while the core audit's findings stand.
//!
//! None is implemented yet: each is tracked by its own issue ([`ProviderSpec::issue`]), and until
//! it lands, enabling it reports it unavailable in this build.

use std::fmt;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// An optional provider. Only the registry ([`PROVIDERS`]) makes them, so every `ProviderId` is
/// known. The names are stable.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProviderId(&'static str);

impl ProviderId {
    /// Prose linting with Vale.
    pub const VALE: Self = Self("vale");
    /// Requests to the workspace's external links.
    pub const EXTERNAL_LINKS: Self = Self("external-links");
    /// What the native renderer and PDF export do not support.
    pub const COMPATIBILITY: Self = Self("compatibility");
    /// How long ago each page last changed, from Git history.
    pub const PAGE_AGE: Self = Self("page-age");

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The registered provider named `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        PROVIDERS
            .iter()
            .map(|provider| provider.id)
            .find(|id| id.0 == name)
    }

    /// The provider's entry in the registry.
    #[must_use]
    pub fn spec(self) -> &'static ProviderSpec {
        PROVIDERS
            .iter()
            .find(|provider| provider.id == self)
            .expect("every ProviderId is registered")
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Serialize for ProviderId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for ProviderId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::from_name(&name)
            .ok_or_else(|| de::Error::custom(format!("unknown provider `{name}`")))
    }
}

/// What a provider needs that the core audit never uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Needs {
    /// An external program, by the name it is found by.
    Tool(&'static str),
    /// Requests over the network.
    Network,
    /// The repository's history.
    GitHistory,
    /// Nothing beyond the workspace.
    Workspace,
}

/// A provider in the registry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderSpec {
    pub id: ProviderId,
    pub summary: &'static str,
    pub needs: Needs,
    /// The issue it is tracked by.
    pub issue: u32,
}

/// Every provider, in name order.
pub const PROVIDERS: [ProviderSpec; 4] = [
    ProviderSpec {
        id: ProviderId::COMPATIBILITY,
        summary: "Constructs the native renderer approximates and theme keys PDF export ignores",
        needs: Needs::Workspace,
        issue: 104,
    },
    ProviderSpec {
        id: ProviderId::EXTERNAL_LINKS,
        summary: "External links that no longer answer",
        needs: Needs::Network,
        issue: 86,
    },
    ProviderSpec {
        id: ProviderId::PAGE_AGE,
        summary: "Pages not changed in a long time, from Git history",
        needs: Needs::GitHistory,
        issue: 289,
    },
    ProviderSpec {
        id: ProviderId::VALE,
        summary: "Prose style alerts from Vale",
        needs: Needs::Tool("vale"),
        issue: 85,
    },
];

/// What became of a provider in an audit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ProviderState {
    /// The policy does not enable it.
    Disabled,
    /// Enabled, and it ran over the whole scope.
    Ran,
    /// Enabled, but it could not run, for `reason`.
    Unavailable { reason: String },
}

/// A provider's state in a report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub provider: ProviderId,
    #[serde(flatten)]
    pub state: ProviderState,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_ordered_and_named_stably() {
        let names: Vec<&str> = PROVIDERS
            .iter()
            .map(|provider| provider.id.as_str())
            .collect();
        assert_eq!(
            names,
            ["compatibility", "external-links", "page-age", "vale"]
        );
        for provider in PROVIDERS {
            assert_eq!(
                ProviderId::from_name(provider.id.as_str()),
                Some(provider.id)
            );
            assert_eq!(provider.id.spec(), &provider);
        }
        assert_eq!(ProviderId::from_name("spellcheck"), None);
        assert_eq!(ProviderId::VALE.spec().needs, Needs::Tool("vale"));
    }

    #[test]
    fn statuses_serialise_with_their_state() {
        let status = ProviderStatus {
            provider: ProviderId::VALE,
            state: ProviderState::Unavailable {
                reason: "`vale` is not on PATH".to_owned(),
            },
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "provider": "vale",
                "state": "unavailable",
                "reason": "`vale` is not on PATH",
            })
        );
        assert_eq!(
            serde_json::from_value::<ProviderStatus>(json).unwrap(),
            status
        );
    }
}
