//! Updates, compared with releases on the Gitea instance (D27, D76).
//!
//! A tagged build looks at the latest release (prereleases excluded) and
//! compares versions. A nightly build (`.gitea/workflows/nightly.yml` sets
//! `IRCHAD_NIGHTLY` to the commit at compile time) looks at the rolling
//! `nightly` prerelease and compares commits.
//!
//! Where IrChad may replace its own program file ([`blocker`] is `None`), a
//! newer build is downloaded, checked against `packaging/update.pub` and the
//! `VERSION` inside it, and staged; "Restart to update" swaps it in
//! ([`install`]) and `main` relaunches it once the window has closed
//! ([`relaunch`]). Anywhere else it's only a notice with a link.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::AsyncWriteExt;

const REPO: &str = "https://api.github.com/repos/irchad-chat/irchad";
/// Release pages we link to; anything else from the server is dropped.
const PAGES: &str = "https://github.com/irchad-chat/irchad/releases/";
/// Where update archives may come from; any other URL is refused.
const DOWNLOADS: &str = "https://github.com/irchad-chat/irchad/releases/download/";

/// The zipsign key CI signs update archives with (D76).
const KEY: self_update::VerifyingKey = *include_bytes!("../../../packaging/update.pub");
/// Bigger than any build we ship (25 MiB) by a wide margin.
const MAX_DOWNLOAD: u64 = 256 << 20;
/// How long until the next check while nothing newer is out: anywhere in 1–6 h,
/// so clients don't all ask the release server at once.
fn recheck() -> Duration {
    use std::hash::{BuildHasher, Hasher};
    // std's per-process random keys: plenty for jitter, no RNG crate needed.
    let r = std::hash::RandomState::new().build_hasher().finish();
    Duration::from_secs(60 * 60 + r % (5 * 60 * 60 + 1))
}

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

/// The nightly asset for this platform: a nightly without it isn't
/// finished (or, on Gitea, not uploaded yet) and isn't offered.
const ASSET: Option<&str> = if cfg!(target_os = "linux") {
    Some("irchad-linux-x86_64.tar.gz")
} else if cfg!(target_os = "windows") {
    Some("irchad-windows-x86_64-setup.exe")
} else {
    None
};

/// The signed archive this platform updates from, and where its program
/// and `VERSION` line are inside it (`packaging/linux/tarball.sh`,
/// `packaging/windows/update-zip.ps1`).
struct Package {
    asset: &'static str,
    exe: &'static str,
    version: &'static str,
}

const PACKAGE: Option<Package> = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some(Package {
        asset: "irchad-linux-x86_64.tar.gz",
        exe: "irchad/irchad",
        version: "irchad/VERSION",
    })
} else if cfg!(all(windows, target_arch = "x86_64")) {
    Some(Package {
        asset: "irchad-windows-x86_64.zip",
        exe: "irchad.exe",
        version: "VERSION",
    })
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
    #[serde(default)]
    browser_download_url: String,
}

/// A newer build than this one.
struct Available {
    /// For people: `0.1.25` or `nightly f78602ee8`.
    version: String,
    page: String,
    /// The `VERSION` line its archive must carry: `0.1.25` or
    /// `nightly <the full commit>`.
    expected: String,
    /// Its update archive for this platform, if it has one.
    download: Option<String>,
}

/// What the banner shows.
#[derive(Clone)]
pub enum Status {
    /// Newer, with a link to its page.
    Out { version: String, page: String },
    /// Newer, downloaded and checked: the program [`install`] swaps in.
    Ready {
        version: String,
        page: String,
        exe: PathBuf,
    },
}

/// Looks for a newer build (again every 1–6 h until there is one), and
/// with `auto` downloads and stages it. `dir` is ours to empty.
pub async fn run(
    auto: bool,
    dir: PathBuf,
    tx: std::sync::mpsc::Sender<Status>,
    ctx: egui::Context,
) {
    // Whatever an earlier run staged but never installed.
    let _ = tokio::fs::remove_dir_all(&dir).await;
    let found = loop {
        if let Some(a) = check().await {
            break a;
        }
        tokio::time::sleep(recheck()).await;
    };
    let status = match found.download.as_deref() {
        Some(url) if auto => match stage(url, &found.expected, &dir).await {
            Ok(exe) => {
                tracing::info!("update to {} staged", found.version);
                Status::Ready {
                    version: found.version,
                    page: found.page,
                    exe,
                }
            }
            Err(e) => {
                tracing::warn!("couldn't download {}: {e}", found.version);
                Status::Out {
                    version: found.version,
                    page: found.page,
                }
            }
        },
        _ => Status::Out {
            version: found.version,
            page: found.page,
        },
    };
    let _ = tx.send(status);
    ctx.request_repaint();
}

/// `Some` if there is something newer than this build.
async fn check() -> Option<Available> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("irchad/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(15))
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
    let (version, expected) = match NIGHTLY {
        Some(ours) => {
            let label = newer_nightly(&r, ours, ASSET?)?;
            (label, format!("nightly {}", r.target_commitish))
        }
        None => {
            newer(&r.tag_name, env!("CARGO_PKG_VERSION")).then_some(())?;
            let v = r.tag_name.trim_start_matches('v').to_owned();
            (v.clone(), v)
        }
    };
    Some(Available {
        version,
        expected,
        download: PACKAGE
            .as_ref()
            .and_then(|p| download_url(&r, p.asset))
            .map(str::to_owned),
        page: r.html_url,
    })
}

/// `asset`'s download URL in `r`, if it's one of ours.
fn download_url<'a>(r: &'a Release, asset: &str) -> Option<&'a str> {
    let url = &r
        .assets
        .iter()
        .find(|a| a.name == asset)?
        .browser_download_url;
    (url.starts_with(DOWNLOADS) && url.ends_with(&format!("/{asset}"))).then_some(url)
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

/// Why this copy of IrChad can't update itself, for the setting's tooltip;
/// `None` when it can.
pub fn blocker() -> Option<&'static str> {
    static BLOCKER: OnceLock<Option<&'static str>> = OnceLock::new();
    *BLOCKER.get_or_init(|| {
        if PACKAGE.is_none() {
            Some("IrChad can't update itself on this platform.")
        } else if NIGHTLY.is_none() && COMMIT.is_some() {
            Some("Development builds don't update themselves.")
        } else if std::env::var_os("FLATPAK_ID").is_some() {
            Some("Flatpak keeps IrChad up to date.")
        } else if !writable() {
            Some(
                "IrChad can't replace its own program file, so it was probably \
                 installed by a package manager. Update it there.",
            )
        } else {
            None
        }
    })
}

/// Whether we can replace our own program: its folder takes a new file
/// (replacing is a rename there), and on Unix that file would be ours like
/// the program is, so a root-run copy in `/usr/bin` doesn't count.
fn writable() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    let probe = dir.join(format!(".irchad-update-{}", std::process::id()));
    let Ok(file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    else {
        return false;
    };
    #[cfg(unix)]
    let ours = {
        use std::os::unix::fs::MetadataExt;
        match (file.metadata(), exe.metadata()) {
            (Ok(p), Ok(e)) => p.uid() == e.uid() && e.uid() != 0,
            _ => false,
        }
    };
    #[cfg(not(unix))]
    let ours = true;
    drop(file);
    let _ = std::fs::remove_file(&probe);
    ours
}

/// Downloads `url` into `dir` and checks it; the staged program on success.
async fn stage(url: &str, expected: &str, dir: &Path) -> Result<PathBuf, String> {
    let package = PACKAGE.as_ref().ok_or("no update for this platform")?;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    // Under the asset's own name: it's the signature's context.
    let archive = dir.join(package.asset);
    download(url, &archive).await?;
    let (dir, expected) = (dir.to_owned(), expected.to_owned());
    tokio::task::spawn_blocking(move || {
        unpack(package, &archive, &dir.join("new"), &expected, &[KEY])
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn download(url: &str, to: &Path) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("irchad/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .timeout(Duration::from_secs(30 * 60))
        .build()
        .map_err(|e| e.to_string())?;
    let mut r = client
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?;
    if r.content_length().is_some_and(|n| n > MAX_DOWNLOAD) {
        return Err("too big".into());
    }
    let mut file = tokio::fs::File::create(to)
        .await
        .map_err(|e| format!("{}: {e}", to.display()))?;
    let mut total = 0u64;
    while let Some(chunk) = r.chunk().await.map_err(|e| e.to_string())? {
        total += chunk.len() as u64;
        if total > MAX_DOWNLOAD {
            return Err("too big".into());
        }
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
    }
    file.sync_all().await.map_err(|e| e.to_string())
}

/// Checks `archive`'s signature against `keys` and its `VERSION` against
/// `expected`, then extracts its program into `into`.
fn unpack(
    package: &Package,
    archive: &Path,
    into: &Path,
    expected: &str,
    keys: &[self_update::VerifyingKey],
) -> Result<PathBuf, String> {
    self_update::verify_signature(archive, keys).map_err(|e| format!("bad signature: {e}"))?;
    // Signed by us from here on, but an old signed build served as new
    // would pass that too.
    // tar's unpacking wants the folder there already.
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    let extract = self_update::Extract::from_source(archive);
    extract
        .extract_file(into, package.version)
        .map_err(|e| format!("no VERSION: {e}"))?;
    let mut found = String::new();
    std::fs::File::open(into.join(package.version))
        .and_then(|f| f.take(256).read_to_string(&mut found))
        .map_err(|e| format!("VERSION: {e}"))?;
    if found.trim() != expected {
        return Err(format!(
            "archive is {:?}, expected {expected:?}",
            found.trim()
        ));
    }
    extract
        .extract_file(into, package.exe)
        .map_err(|e| format!("no program: {e}"))?;
    let exe = into.join(package.exe);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    Ok(exe)
}

/// The program to start once we've exited, set by [`install`].
static RELAUNCH: OnceLock<PathBuf> = OnceLock::new();

/// Puts the staged program `exe` in place of ours. The running one carries
/// on; close the window and `main` [`relaunch`]es.
pub fn install(exe: &Path) -> Result<(), String> {
    // Before the swap: on Linux it would name the old, unlinked file after.
    let ours = std::env::current_exe().map_err(|e| e.to_string())?;
    self_replace::self_replace(exe).map_err(|e| e.to_string())?;
    let _ = RELAUNCH.set(ours);
    Ok(())
}

/// Whether [`install`] has put a new program in place.
pub fn installed() -> bool {
    RELAUNCH.get().is_some()
}

/// Starts the program [`install`] put in place, with no arguments. Call it
/// last, once the single-instance lock is gone, or the new one would hand
/// over to us and exit. On Unix it replaces this process and only returns
/// on failure.
pub fn relaunch() {
    let Some(exe) = RELAUNCH.get() else {
        return;
    };
    let mut cmd = std::process::Command::new(exe);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let e = cmd.exec();
        tracing::error!("couldn't restart {}: {e}", exe.display());
    }
    #[cfg(not(unix))]
    if let Err(e) = cmd.spawn() {
        tracing::error!("couldn't restart {}: {e}", exe.display());
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Asset, Duration, Release, recheck};

    #[test]
    fn recheck_is_one_to_six_hours() {
        let all: Vec<Duration> = (0..200).map(|_| recheck()).collect();
        assert!(all.iter().all(|d| (3600..=6 * 3600).contains(&d.as_secs())));
        assert!(all.iter().any(|d| *d != all[0]), "not random");
    }

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
                .map(|&name| Asset {
                    name: name.into(),
                    browser_download_url: format!("{}nightly/{name}", super::DOWNLOADS),
                })
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

    #[test]
    fn download_urls() {
        let mut r = nightly(A, &["a.zip"]);
        assert!(super::download_url(&r, "a.zip").is_some());
        assert!(super::download_url(&r, "b.zip").is_none());
        // Somewhere else, or another asset's file under this name.
        r.assets[0].browser_download_url = "https://evil.example/a.zip".into();
        assert!(super::download_url(&r, "a.zip").is_none());
        r.assets[0].browser_download_url = format!("{}nightly/b.zip", super::DOWNLOADS);
        assert!(super::download_url(&r, "a.zip").is_none());
    }

    /// A tarball laid out like `packaging/linux/tarball.sh`'s, signed the
    /// way `packaging/sign.sh` does it (context: the file name).
    fn tarball(dir: &std::path::Path, version: &str, key: &zipsign_api::SigningKey) -> PathBuf {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, data) in [
            ("irchad/irchad", b"new program".as_slice()),
            ("irchad/VERSION", format!("{version}\n").as_bytes()),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            tar.append_data(&mut h, path, data).unwrap();
        }
        let gz = tar.into_inner().unwrap().finish().unwrap();
        let mut signed = std::io::Cursor::new(Vec::new());
        zipsign_api::sign::copy_and_sign_tar(
            &mut std::io::Cursor::new(gz),
            &mut signed,
            std::slice::from_ref(key),
            Some(TARBALL.asset.as_bytes()),
        )
        .unwrap();
        let path = dir.join(TARBALL.asset);
        std::fs::write(&path, signed.into_inner()).unwrap();
        path
    }

    const TARBALL: super::Package = super::Package {
        asset: "irchad-linux-x86_64.tar.gz",
        exe: "irchad/irchad",
        version: "irchad/VERSION",
    };

    #[test]
    fn unpack_checks_signature_and_version() {
        let key = zipsign_api::SigningKey::from_bytes(&[7; 32]);
        let other = zipsign_api::SigningKey::from_bytes(&[8; 32]);
        let keys = [key.verifying_key().to_bytes()];
        let p = &TARBALL;
        let unpack = |archive: &std::path::Path, expected| {
            let into = archive.parent().unwrap().join("new");
            super::unpack(p, archive, &into, expected, &keys)
        };

        let dir = tempfile::tempdir().unwrap();
        let good = tarball(dir.path(), "nightly abc", &key);
        let exe = unpack(&good, "nightly abc").unwrap();
        assert_eq!(std::fs::read(exe).unwrap(), b"new program");

        // An older (or any other) signed build served as the one we expected.
        let e = unpack(&good, "0.2.0").unwrap_err();
        assert!(e.contains("expected"), "{e}");

        // Signed with a key we don't trust.
        let dir = tempfile::tempdir().unwrap();
        let forged = tarball(dir.path(), "0.2.0", &other);
        let e = unpack(&forged, "0.2.0").unwrap_err();
        assert!(e.starts_with("bad signature"), "{e}");

        // Signed for another file name.
        let renamed = dir.path().join("other.tar.gz");
        std::fs::copy(tarball(dir.path(), "0.2.0", &key), &renamed).unwrap();
        let e = unpack(&renamed, "0.2.0").unwrap_err();
        assert!(e.starts_with("bad signature"), "{e}");
    }
}
