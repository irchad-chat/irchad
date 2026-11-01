//! "A new version is out", compared with releases on the Gitea instance.
//! Only a notice; installing is up to the user.
//!
//! A tagged build looks at the latest release (prereleases excluded) and
//! compares versions. A nightly build (`.gitea/workflows/nightly.yml` sets
//! `IRCHAD_NIGHTLY` to the commit at compile time) looks at the rolling
//! `nightly` prerelease and compares commits.

use serde::Deserialize;

const REPO: &str = "https://git.gentoo.party/api/v1/repos/sam/irchad-ng";
/// Release pages we link to; anything else from the server is dropped.
const PAGES: &str = "https://git.gentoo.party/sam/irchad-ng/releases/";

/// The commit this nightly was built from; `None` for any other build.
const NIGHTLY: Option<&str> = option_env!("IRCHAD_NIGHTLY");

/// The commit built, unless it's the release tag (see `build.rs`).
const COMMIT: Option<&str> = option_env!("IRCHAD_COMMIT");

/// What this build is: `0.1.1` for the release, `nightly f78602ee8`, or
/// `dev f78602ee8` for anything else built from git.
pub fn version() -> String {
    let short = |sha: &'static str| sha.get(..9).unwrap_or(sha);
    match (NIGHTLY, COMMIT) {
        (Some(sha), _) => format!("nightly {}", short(sha)),
        (None, Some(sha)) => format!("dev {}", short(sha)),
        (None, None) => env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// The nightly asset for this platform. The workflow recreates the release
/// before building, so its commit is new before the downloads are there.
const ASSET: Option<&str> = if cfg!(target_os = "linux") {
    Some("irchad-linux-x86_64")
} else if cfg!(target_os = "windows") {
    // Uploaded last by the Windows job.
    Some("irchad-windows-x86_64-setup.exe")
} else {
    None
};

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    target_commitish: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
}

/// `Some((version, page))` if there is something newer than this build.
pub async fn check() -> Option<(String, String)> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .https_only(true)
        .build()
        .ok()?;
    let url = match NIGHTLY {
        Some(_) => format!("{REPO}/releases/tags/nightly"),
        None => format!("{REPO}/releases/latest"),
    };
    let r: Release = client
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    if !r.html_url.starts_with(PAGES) {
        return None;
    }
    let version = match NIGHTLY {
        Some(ours) => newer_nightly(&r, ours, ASSET?)?,
        None => newer(&r.tag_name, env!("CARGO_PKG_VERSION"))
            .then(|| r.tag_name.trim_start_matches('v').to_owned())?,
    };
    Some((version, r.html_url))
}

/// The label for a nightly built from a different commit than ours, once
/// `asset` has been uploaded to it.
fn newer_nightly(r: &Release, ours: &str, asset: &str) -> Option<String> {
    let theirs = &r.target_commitish;
    let sha = |s: &str| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit());
    if !sha(theirs) || !sha(ours) || theirs.eq_ignore_ascii_case(ours) {
        return None;
    }
    r.assets.iter().any(|a| a.name == asset).then_some(())?;
    Some(format!("nightly {}", &theirs[..9]))
}

/// Dotted numeric compare; anything unparseable is not newer.
fn newer(tag: &str, ours: &str) -> bool {
    let parse = |s: &str| -> Option<Vec<u64>> {
        s.trim_start_matches('v')
            .split('.')
            .map(|p| p.parse().ok())
            .collect()
    };
    match (parse(tag), parse(ours)) {
        (Some(t), Some(o)) => t > o,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{Asset, Release};

    #[test]
    fn versions() {
        assert!(super::newer("v0.2.0", "0.1.9"));
        assert!(super::newer("0.10.0", "0.9.0"));
        assert!(!super::newer("v0.1.0", "0.1.0"));
        assert!(!super::newer("nightly", "0.1.0"));
    }

    const A: &str = "f78602ee8c3b61a1f08f4f5a1598b61e4f9b6093";
    const B: &str = "956418f0000000000000000000000000000000ab";

    fn nightly(commit: &str, assets: &[&str]) -> Release {
        Release {
            tag_name: "nightly".into(),
            html_url: String::new(),
            target_commitish: commit.into(),
            assets: assets
                .iter()
                .map(|&name| Asset { name: name.into() })
                .collect(),
        }
    }

    #[test]
    fn nightlies() {
        let check = |r: &Release, ours| super::newer_nightly(r, ours, "x");
        assert_eq!(
            check(&nightly(A, &["x"]), B).as_deref(),
            Some("nightly f78602ee8")
        );
        // Same commit, either case.
        assert_eq!(check(&nightly(A, &["x"]), A), None);
        assert_eq!(check(&nightly(&A.to_uppercase(), &["x"]), A), None);
        // Still building, or not built for us.
        assert_eq!(check(&nightly(A, &[]), B), None);
        assert_eq!(check(&nightly(A, &["y"]), B), None);
        // Not a commit hash (a branch name, or something hostile).
        assert_eq!(check(&nightly("main", &["x"]), B), None);
        assert_eq!(check(&nightly("é".repeat(20).as_str(), &["x"]), B), None);
    }
}
