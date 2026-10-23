//! Links to a line of a file on the site that hosts the repository's remote.

use std::collections::BTreeMap;

use crate::Head;

/// The kind of hosting site, which decides how a link to a line is written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostKind {
    GitHub,
    GitLab,
    Bitbucket,
    Azure,
}

impl HostKind {
    /// `github`, `gitlab`, `bitbucket` or `azure`, as settings write it.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "github" => Some(Self::GitHub),
            "gitlab" => Some(Self::GitLab),
            "bitbucket" => Some(Self::Bitbucket),
            "azure" => Some(Self::Azure),
            _ => None,
        }
    }
}

/// The URL of 1-based line `line` of `path` (relative to the repository root, `/`-separated) on
/// the site hosting `remote_url`, at the current branch, or at the commit when `HEAD` is
/// detached. GitHub, GitLab, Bitbucket Cloud and Azure DevOps are recognised by their host
/// names, other hosts through `hosts` (host name → kind); anything else, and an unborn branch,
/// has no link.
#[must_use]
pub fn remote_link_for_line(
    remote_url: &str,
    head: &Head,
    path: &str,
    line: usize,
    hosts: &BTreeMap<String, HostKind>,
) -> Option<String> {
    let (host, repository) = split_remote(remote_url)?;
    let kind = hosts.get(&host).copied().or_else(|| known_host(&host))?;
    let (reference, is_commit) = match head {
        Head::Branch(name) => (name.clone(), false),
        Head::Detached(id) => (id.clone(), true),
        Head::Unborn => return None,
    };
    let path = encode(path);
    Some(match kind {
        // `?plain=1` shows the source, so the line anchor works on rendered files like AsciiDoc.
        HostKind::GitHub => {
            format!("https://{host}/{repository}/blob/{reference}/{path}?plain=1#L{line}")
        }
        HostKind::GitLab => {
            format!("https://{host}/{repository}/-/blob/{reference}/{path}#L{line}")
        }
        HostKind::Bitbucket => {
            format!("https://{host}/{repository}/src/{reference}/{path}#lines-{line}")
        }
        HostKind::Azure => {
            let (organisation, project, name) = azure_parts(&repository)?;
            let version = if is_commit { "GC" } else { "GB" };
            format!(
                "https://dev.azure.com/{organisation}/{project}/_git/{name}?path=/{path}\
                 &version={version}{reference}&line={line}&lineEnd={line}\
                 &lineStartColumn=1&lineEndColumn=1"
            )
        }
    })
}

fn known_host(host: &str) -> Option<HostKind> {
    match host {
        "github.com" => Some(HostKind::GitHub),
        "gitlab.com" => Some(HostKind::GitLab),
        "bitbucket.org" => Some(HostKind::Bitbucket),
        "dev.azure.com" | "ssh.dev.azure.com" => Some(HostKind::Azure),
        _ if host.ends_with(".visualstudio.com") => Some(HostKind::Azure),
        _ => None,
    }
}

/// The host and the repository's path on it, from `git@host:org/repo.git`,
/// `ssh://git@host:22/org/repo.git` or `https://host/org/repo` forms.
fn split_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        let rest = rest.split_once('@').map_or(rest, |(_, rest)| rest);
        let (authority, path) = rest.split_once('/')?;
        let host = authority
            .split_once(':')
            .map_or(authority, |(host, _)| host);
        (host, path)
    } else {
        let rest = url.split_once('@').map_or(url, |(_, rest)| rest);
        rest.split_once(':')?
    };
    let path = path.trim_start_matches('/');
    (!host.is_empty() && !path.is_empty()).then(|| (host.to_lowercase(), path.to_owned()))
}

/// Azure DevOps' organisation, project and repository, from `org/project/_git/repo` (HTTPS) or
/// `v3/org/project/repo` (SSH).
fn azure_parts(path: &str) -> Option<(String, String, String)> {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["v3", organisation, project, name] | [organisation, project, "_git", name] => Some((
            (*organisation).to_owned(),
            (*project).to_owned(),
            (*name).to_owned(),
        )),
        _ => None,
    }
}

/// Escapes what a path may hold that a URL cannot.
fn encode(path: &str) -> String {
    path.replace('%', "%25")
        .replace(' ', "%20")
        .replace('#', "%23")
        .replace('?', "%3F")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(name: &str) -> Head {
        Head::Branch(name.to_owned())
    }

    #[test]
    fn remote_link_github_ssh_and_https() {
        let hosts = BTreeMap::new();
        let expected =
            "https://github.com/bovinemagnet/AdocFoundry/blob/main/README.md?plain=1#L12";
        for url in [
            "git@github.com:bovinemagnet/AdocFoundry.git",
            "https://github.com/bovinemagnet/AdocFoundry.git",
            "https://github.com/bovinemagnet/AdocFoundry",
            "ssh://git@github.com/bovinemagnet/AdocFoundry.git",
        ] {
            assert_eq!(
                remote_link_for_line(url, &branch("main"), "README.md", 12, &hosts).as_deref(),
                Some(expected),
                "{url}"
            );
        }
    }

    #[test]
    fn remote_link_gitlab_self_hosted_uses_dash_blob() {
        let hosts = BTreeMap::from([("git.example.com".to_owned(), HostKind::GitLab)]);
        assert_eq!(
            remote_link_for_line(
                "git@git.example.com:docs/handbook.git",
                &branch("release/2.0"),
                "modules/ROOT/pages/index.adoc",
                3,
                &hosts
            )
            .as_deref(),
            Some(
                "https://git.example.com/docs/handbook/-/blob/release/2.0/modules/ROOT/pages/index.adoc#L3"
            )
        );
        assert_eq!(
            remote_link_for_line(
                "https://gitlab.com/group/sub/repo.git",
                &branch("main"),
                "a b.adoc",
                1,
                &BTreeMap::new()
            )
            .as_deref(),
            Some("https://gitlab.com/group/sub/repo/-/blob/main/a%20b.adoc#L1")
        );
    }

    #[test]
    fn remote_link_unknown_host_is_none() {
        let hosts = BTreeMap::new();
        assert_eq!(
            remote_link_for_line(
                "git@git.example.com:docs/handbook.git",
                &branch("main"),
                "a",
                1,
                &hosts
            ),
            None
        );
        assert_eq!(
            remote_link_for_line("/srv/git/docs.git", &branch("main"), "a", 1, &hosts),
            None
        );
        assert_eq!(
            remote_link_for_line("git@github.com:org/repo.git", &Head::Unborn, "a", 1, &hosts),
            None
        );
    }

    #[test]
    fn remote_link_detached_head_uses_commit() {
        let head = Head::Detached("1a2b3c4d5e6f".to_owned());
        let hosts = BTreeMap::new();
        assert_eq!(
            remote_link_for_line("git@github.com:org/repo.git", &head, "page.adoc", 7, &hosts)
                .as_deref(),
            Some("https://github.com/org/repo/blob/1a2b3c4d5e6f/page.adoc?plain=1#L7")
        );
        assert_eq!(
            remote_link_for_line(
                "git@bitbucket.org:team/repo.git",
                &head,
                "page.adoc",
                7,
                &hosts
            )
            .as_deref(),
            Some("https://bitbucket.org/team/repo/src/1a2b3c4d5e6f/page.adoc#lines-7")
        );
        let azure = remote_link_for_line(
            "git@ssh.dev.azure.com:v3/acme/Docs/handbook",
            &head,
            "page.adoc",
            7,
            &hosts,
        )
        .unwrap();
        assert!(azure.starts_with("https://dev.azure.com/acme/Docs/_git/handbook?path=/page.adoc"));
        assert!(azure.contains("&version=GC1a2b3c4d5e6f&line=7"));
    }
}
