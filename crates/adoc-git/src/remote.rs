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
    let remote = split_remote(remote_url)?;
    let kind = hosts
        .get(&remote.host)
        .copied()
        .or_else(|| known_host(&remote.host))?;
    let (reference, is_commit) = match head {
        Head::Branch(name) => (name.as_str(), false),
        Head::Detached(id) => (id.as_str(), true),
        Head::Unborn => return None,
    };
    let site = &remote.site;
    let repository = &remote.path;
    Some(match kind {
        // `?plain=1` shows the source, so the line anchor works on rendered files like AsciiDoc.
        HostKind::GitHub => format!(
            "{site}/{repository}/blob/{}/{}?plain=1#L{line}",
            encode(reference),
            encode(path)
        ),
        HostKind::GitLab => format!(
            "{site}/{repository}/-/blob/{}/{}#L{line}",
            encode(reference),
            encode(path)
        ),
        HostKind::Bitbucket => format!(
            "{site}/{repository}/src/{}/{}#lines-{line}",
            encode(reference),
            encode(path)
        ),
        HostKind::Azure => {
            let base = azure_repository(&remote)?;
            let version = if is_commit { "GC" } else { "GB" };
            format!(
                "{base}?path=/{}&version={version}{}&line={line}&lineEnd={line}\
                 &lineStartColumn=1&lineEndColumn=1",
                encode(path),
                encode(reference).replace('/', "%2F")
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

/// A remote, from `git@host:org/repo.git`, `ssh://git@host:22/org/repo.git` or
/// `https://host:8443/org/repo` forms.
struct Remote {
    /// The host name, lower case, which names the kind of site.
    host: String,
    /// The site's address: the scheme and authority of an `http(s)://` remote, port included,
    /// else `https://` and the host.
    site: String,
    /// The repository's path on the host.
    path: String,
}

fn split_remote(url: &str) -> Option<Remote> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let (scheme, authority, path) = if let Some((scheme, rest)) = url.split_once("://") {
        let rest = rest.split_once('@').map_or(rest, |(_, rest)| rest);
        let (authority, path) = rest.split_once('/')?;
        (scheme.to_lowercase(), authority, path)
    } else {
        let rest = url.split_once('@').map_or(url, |(_, rest)| rest);
        let (host, path) = rest.split_once(':')?;
        ("ssh".to_owned(), host, path)
    };
    let host = authority
        .split_once(':')
        .map_or(authority, |(host, _)| host)
        .to_lowercase();
    let path = path.trim_start_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    // A web remote is served where it was fetched from; an SSH port is not the site's.
    let site = match scheme.as_str() {
        "http" | "https" => format!("{scheme}://{}", authority.to_lowercase()),
        _ => format!("https://{host}"),
    };
    Some(Remote {
        host,
        site,
        path: path.to_owned(),
    })
}

/// The address of an Azure DevOps repository. Azure DevOps Services is written at
/// `dev.azure.com`, from `org/project/_git/repo` (HTTPS), `v3/org/project/repo` (SSH) or
/// `[DefaultCollection/]project/_git/repo` under `org.visualstudio.com`; a server named under
/// `[git.hosts]` keeps its own site and the path up to the repository.
fn azure_repository(remote: &Remote) -> Option<String> {
    let parts: Vec<&str> = remote.path.split('/').collect();
    let cloud = |organisation: &str, project: &str, name: &str| {
        Some(format!(
            "https://dev.azure.com/{organisation}/{project}/_git/{name}"
        ))
    };
    if let Some(organisation) = remote.host.strip_suffix(".visualstudio.com") {
        return match parts.as_slice() {
            ["v3", organisation, project, name] => cloud(organisation, project, name),
            ["DefaultCollection", project, "_git", name] | [project, "_git", name] => {
                cloud(organisation, project, name)
            }
            _ => None,
        };
    }
    match parts.as_slice() {
        ["v3", organisation, project, name] => cloud(organisation, project, name),
        [organisation, project, "_git", name]
            if matches!(remote.host.as_str(), "dev.azure.com" | "ssh.dev.azure.com") =>
        {
            cloud(organisation, project, name)
        }
        [.., "_git", _] => Some(format!("{}/{}", remote.site, remote.path)),
        _ => None,
    }
}

/// Percent-encodes every byte of `text` but unreserved characters and the `/` between segments,
/// so a name may hold anything a path or a query value cannot.
fn encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
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
    fn remote_links_escape_names_and_keep_the_site() {
        // Issue #556.
        let hosts = BTreeMap::from([
            ("tfs.example.com".to_owned(), HostKind::Azure),
            ("git.example.com".to_owned(), HostKind::GitLab),
        ]);
        let query = "&lineStartColumn=1&lineEndColumn=1";
        for (url, reference, path, expected) in [
            (
                "git@github.com:org/repo.git",
                "fix/#123-typo",
                "a.adoc",
                "https://github.com/org/repo/blob/fix/%23123-typo/a.adoc?plain=1#L3".to_owned(),
            ),
            (
                "git@github.com:org/repo.git",
                "50%-off",
                "é [1].adoc",
                "https://github.com/org/repo/blob/50%25-off/%C3%A9%20%5B1%5D.adoc?plain=1#L3"
                    .to_owned(),
            ),
            (
                "https://dev.azure.com/acme/Docs/_git/handbook",
                "r&d",
                "q&a/a+b.adoc",
                format!(
                    "https://dev.azure.com/acme/Docs/_git/handbook?path=/q%26a/a%2Bb.adoc\
                     &version=GBr%26d&line=3&lineEnd=3{query}"
                ),
            ),
            (
                "https://acme.visualstudio.com/Docs/_git/handbook",
                "main",
                "a.adoc",
                format!(
                    "https://dev.azure.com/acme/Docs/_git/handbook?path=/a.adoc\
                     &version=GBmain&line=3&lineEnd=3{query}"
                ),
            ),
            (
                "https://acme.visualstudio.com/DefaultCollection/Docs/_git/handbook",
                "main",
                "a.adoc",
                format!(
                    "https://dev.azure.com/acme/Docs/_git/handbook?path=/a.adoc\
                     &version=GBmain&line=3&lineEnd=3{query}"
                ),
            ),
            (
                "https://tfs.example.com/tfs/Docs/_git/handbook",
                "main",
                "a.adoc",
                format!(
                    "https://tfs.example.com/tfs/Docs/_git/handbook?path=/a.adoc\
                     &version=GBmain&line=3&lineEnd=3{query}"
                ),
            ),
            (
                "https://git.example.com:8443/docs/handbook.git",
                "main",
                "a.adoc",
                "https://git.example.com:8443/docs/handbook/-/blob/main/a.adoc#L3".to_owned(),
            ),
            (
                "http://git.example.com/docs/handbook.git",
                "main",
                "a.adoc",
                "http://git.example.com/docs/handbook/-/blob/main/a.adoc#L3".to_owned(),
            ),
            (
                "ssh://git@git.example.com:2222/docs/handbook.git",
                "main",
                "a.adoc",
                "https://git.example.com/docs/handbook/-/blob/main/a.adoc#L3".to_owned(),
            ),
        ] {
            assert_eq!(
                remote_link_for_line(url, &branch(reference), path, 3, &hosts).as_deref(),
                Some(expected.as_str()),
                "{url} {reference} {path}"
            );
        }
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
