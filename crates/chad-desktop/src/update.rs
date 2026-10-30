//! "A new version is out": the latest tagged release on the Gitea instance,
//! compared with ours. Only a notice; installing is up to the user.

use serde::Deserialize;

const LATEST: &str = "https://git.gentoo.party/api/v1/repos/sam/irchad-ng/releases/latest";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
}

/// `Some((version, page))` if the latest release is newer than this build.
pub async fn check() -> Option<(String, String)> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .https_only(true)
        .build()
        .ok()?;
    let r: Release = client
        .get(LATEST)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    newer(&r.tag_name, env!("CARGO_PKG_VERSION")).then(|| {
        let v = r.tag_name.trim_start_matches('v').to_owned();
        (v, r.html_url)
    })
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
    #[test]
    fn versions() {
        assert!(super::newer("v0.2.0", "0.1.9"));
        assert!(super::newer("0.10.0", "0.9.0"));
        assert!(!super::newer("v0.1.0", "0.1.0"));
        assert!(!super::newer("nightly", "0.1.0"));
    }
}
