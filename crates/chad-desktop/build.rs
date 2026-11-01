//! Embeds the commit being built as `IRCHAD_COMMIT`, unless it's the release
//! tag `v<version>` (then the version alone says it). Without git, e.g. from a
//! source tarball, nothing is set and the build reports its version.

use std::path::PathBuf;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let Some(head) = git(&["rev-parse", "HEAD"]) else {
        return;
    };
    // Rebuild when HEAD moves: a checkout, a commit on the branch, a new tag.
    if let (Some(dir), Some(common)) = (
        git(&["rev-parse", "--absolute-git-dir"]),
        git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]),
    ) {
        let (dir, common) = (PathBuf::from(dir), PathBuf::from(common));
        println!("cargo:rerun-if-changed={}", dir.join("HEAD").display());
        println!(
            "cargo:rerun-if-changed={}",
            common.join("packed-refs").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            common.join("refs/tags").display()
        );
        if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={}", common.join(r).display());
        }
    }
    let release = format!(
        "v{}",
        std::env::var("CARGO_PKG_VERSION").unwrap_or_default()
    );
    let tags = git(&["tag", "--points-at", "HEAD"]).unwrap_or_default();
    if !tags.lines().any(|t| t == release) {
        println!("cargo:rustc-env=IRCHAD_COMMIT={head}");
    }
}
