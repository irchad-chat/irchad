//! Self-sandboxing (D93): a seccomp denylist and Landlock write rules, put on
//! the process before any thread starts so every thread and child inherits
//! them. Neither stops an exploit, they limit what one does next: no kernel
//! surface a chat client never needs, and no writes outside our own files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use landlock::{ABI, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, path_beneath_rules};
use seccompiler::{
    SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter, SeccompRule,
};

/// Sandboxes this process, unless `IRCHAD_NO_SANDBOX` is set or we're in a
/// Flatpak (bubblewrap already filters syscalls, and the manifest limits
/// files). Returns what happened, for the log once it's up.
pub fn sandbox(data_dir: &Path) -> String {
    if std::env::var_os("IRCHAD_NO_SANDBOX").is_some() {
        return "sandbox: off (IRCHAD_NO_SANDBOX)".into();
    }
    if std::env::var_os("FLATPAK_ID").is_some() {
        return "sandbox: left to Flatpak".into();
    }
    let seccomp = match seccomp() {
        Ok(()) => "on".to_string(),
        Err(e) => format!("failed ({e})"),
    };
    let landlock = match landlock(&writable(data_dir)) {
        Ok(s) => s,
        Err(e) => format!("failed ({e})"),
    };
    SANDBOXED.store(true, std::sync::atomic::Ordering::Relaxed);
    format!("sandbox: seccomp {seccomp}, landlock {landlock}")
}

static SANDBOXED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether [`sandbox`] ran: then a browser we start would inherit it, so
/// links go through the OpenURI portal instead.
pub fn sandboxed() -> bool {
    SANDBOXED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Syscalls that fail with `EPERM`: debugging other processes, namespaces
/// and mounts, kernel keyrings and modules, BPF, io_uring and other kernel
/// surface nothing in IrChad (GPU drivers, libwebrtc, PipeWire) uses.
fn denied() -> Vec<i64> {
    use libc::*;
    let mut v = vec![
        SYS_ptrace,
        SYS_process_vm_readv,
        SYS_process_vm_writev,
        SYS_bpf,
        SYS_perf_event_open,
        SYS_userfaultfd,
        SYS_kexec_load,
        SYS_kexec_file_load,
        SYS_mount,
        SYS_umount2,
        SYS_mount_setattr,
        SYS_pivot_root,
        SYS_chroot,
        SYS_unshare,
        SYS_setns,
        SYS_fsopen,
        SYS_fsconfig,
        SYS_fsmount,
        SYS_fspick,
        SYS_move_mount,
        SYS_open_tree,
        SYS_open_by_handle_at,
        SYS_keyctl,
        SYS_add_key,
        SYS_request_key,
        SYS_init_module,
        SYS_finit_module,
        SYS_delete_module,
        SYS_io_uring_setup,
        SYS_io_uring_enter,
        SYS_io_uring_register,
        SYS_swapon,
        SYS_swapoff,
        SYS_reboot,
        SYS_acct,
        SYS_quotactl,
        SYS_syslog,
        SYS_settimeofday,
        SYS_clock_settime,
        SYS_clock_adjtime,
        SYS_adjtimex,
    ];
    #[cfg(target_arch = "x86_64")]
    v.extend([SYS_iopl, SYS_ioperm]);
    v
}

/// `clone` flags that make namespaces, the usual way to reach kernel code
/// as "root". (`CLONE_NEWTIME` only exists for `clone3` and `unshare`.)
const NAMESPACES: [i32; 7] = [
    libc::CLONE_NEWNS,
    libc::CLONE_NEWCGROUP,
    libc::CLONE_NEWUTS,
    libc::CLONE_NEWIPC,
    libc::CLONE_NEWUSER,
    libc::CLONE_NEWPID,
    libc::CLONE_NEWNET,
];

fn seccomp() -> Result<(), seccompiler::Error> {
    let arch = std::env::consts::ARCH.try_into()?;
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> =
        denied().into_iter().map(|s| (s, vec![])).collect();
    let clone = NAMESPACES
        .iter()
        .map(|&f| {
            let f = f as u64;
            let c =
                SeccompCondition::new(0, SeccompCmpArgLen::Qword, SeccompCmpOp::MaskedEq(f), f)?;
            SeccompRule::new(vec![c])
        })
        .collect::<Result<_, _>>()?;
    rules.insert(libc::SYS_clone, clone);
    let eperm = SeccompAction::Errno(libc::EPERM as u32);
    let deny: seccompiler::BpfProgram =
        SeccompFilter::new(rules, SeccompAction::Allow, eperm, arch)?.try_into()?;
    // clone3's flags sit in a struct a filter can't read, so refuse it whole.
    // ENOSYS, not EPERM: that's what makes glibc fall back to clone.
    let enosys = SeccompAction::Errno(libc::ENOSYS as u32);
    let clone3 = BTreeMap::from([(libc::SYS_clone3, vec![])]);
    let clone3: seccompiler::BpfProgram =
        SeccompFilter::new(clone3, SeccompAction::Allow, enosys, arch)?.try_into()?;
    seccompiler::apply_filter(&deny)?;
    seccompiler::apply_filter(&clone3)
}

/// Where we may still write. Reading stays open everywhere.
fn writable(data_dir: &Path) -> Vec<PathBuf> {
    let home = std::env::home_dir();
    let xdg = |var: &str, fallback: &str| {
        std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.as_ref().map(|h| h.join(fallback)))
    };
    let _ = std::fs::create_dir_all(data_dir);
    let mut paths = vec![
        data_dir.to_owned(),
        // GPU, sound and video devices are opened for writing.
        PathBuf::from("/dev"),
        PathBuf::from("/dev/shm"),
        PathBuf::from("/tmp"),
        PathBuf::from("/var/tmp"),
        std::env::temp_dir(),
        // Thread names go to /proc/self/task/*/comm; resolves to our pid.
        PathBuf::from("/proc/self"),
    ];
    // Shader and font caches (Mesa, Nvidia, fontconfig).
    paths.extend(xdg("XDG_CACHE_HOME", ".cache"));
    paths.extend(std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from));
    paths.extend(downloads());
    // Self-update swaps the program file; the check also writes its probe
    // there, so it has to run now, before we're locked in.
    if crate::update::blocker().is_none() {
        paths.extend(
            std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(Path::to_owned)),
        );
    }
    paths.retain(|p| p.exists());
    paths
}

/// The XDG download folder (`user-dirs.dirs`), else `~/Downloads`: where
/// saved files can go.
pub fn downloads() -> Option<PathBuf> {
    let home = std::env::home_dir()?;
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let dirs = std::fs::read_to_string(config.join("user-dirs.dirs")).unwrap_or_default();
    Some(user_dir(&dirs, "XDG_DOWNLOAD_DIR", &home).unwrap_or_else(|| home.join("Downloads")))
}

/// One `XDG_*_DIR="$HOME/..."` line of `user-dirs.dirs`. The format only
/// allows `$HOME/...` or an absolute path.
fn user_dir(dirs: &str, key: &str, home: &Path) -> Option<PathBuf> {
    let v = dirs
        .lines()
        .find_map(|l| l.trim().strip_prefix(key)?.strip_prefix('='))?;
    let v = v.trim().strip_prefix('"')?.strip_suffix('"')?;
    if let Some(rest) = v.strip_prefix("$HOME") {
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        return (!rest.is_empty() && !rest.split('/').any(|c| c == "..")).then(|| home.join(rest));
    }
    v.starts_with('/').then(|| PathBuf::from(v))
}

fn landlock(paths: &[PathBuf]) -> Result<String, landlock::RulesetError> {
    // V5: up to device ioctls. V9's unix-socket rule would cut off
    // Wayland, PipeWire and D-Bus.
    let write = AccessFs::from_write(ABI::V5);
    let status = Ruleset::default()
        .handle_access(write)?
        .create()?
        .add_rules(path_beneath_rules(paths, write))?
        .restrict_self()?;
    Ok(format!("{:?}", status.ruleset))
}

/// Sends links to the OpenURI portal, so the browser starts outside our
/// sandbox (eframe would run `xdg-open` as our child).
pub struct OpenUrls;

impl egui::Plugin for OpenUrls {
    fn debug_name(&self) -> &'static str {
        "OpenUrls"
    }

    fn output_hook(&mut self, _ctx: &egui::Context, output: &mut egui::FullOutput) {
        output.platform_output.commands.retain(|c| {
            let egui::OutputCommand::OpenUrl(u) = c else {
                return true;
            };
            let url = u.url.clone();
            tokio::spawn(async move {
                let uri = match ashpd::Uri::parse(&url) {
                    Ok(u) => u,
                    Err(e) => return tracing::warn!("not a link: {url}: {e}"),
                };
                let r = ashpd::desktop::open_uri::OpenFileRequest::default()
                    .send_uri(&uri)
                    .await;
                if let Err(e) = r {
                    tracing::warn!("couldn't open {url}: {e}");
                }
            });
            false
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_dirs() {
        let home = Path::new("/home/u");
        let dirs = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_DOWNLOAD_DIR=\"$HOME/Dl\"\n";
        let get = |d: &str| user_dir(d, "XDG_DOWNLOAD_DIR", home);
        assert_eq!(get(dirs), Some(home.join("Dl")));
        assert_eq!(
            get("XDG_DOWNLOAD_DIR=\"/data/dl\""),
            Some("/data/dl".into())
        );
        assert_eq!(get("XDG_DOWNLOAD_DIR=\"$HOME/\""), None);
        assert_eq!(get("XDG_DOWNLOAD_DIR=\"$HOME/../etc\""), None);
        assert_eq!(get("XDG_DOWNLOAD_DIR=\"rel\""), None);
        assert_eq!(get(""), None);
    }

    // Both run on a fresh thread: seccomp and Landlock only bind the thread
    // that asks (and what it starts), so the test runner stays free.

    #[test]
    fn seccomp_denies() {
        std::thread::spawn(|| {
            seccomp().unwrap();
            let err = || std::io::Error::last_os_error().raw_os_error();
            assert_eq!(unsafe { libc::unshare(libc::CLONE_NEWUSER) }, -1);
            assert_eq!(err(), Some(libc::EPERM));
            assert_eq!(unsafe { libc::syscall(libc::SYS_clone3, 0, 0) }, -1);
            assert_eq!(err(), Some(libc::ENOSYS));
            // Threads and children still start (glibc falls back to clone).
            std::thread::spawn(|| ()).join().unwrap();
            assert!(
                std::process::Command::new("true")
                    .status()
                    .unwrap()
                    .success()
            );
        })
        .join()
        .unwrap();
    }

    #[test]
    fn landlock_limits_writes() {
        let ok = tempfile::tempdir().unwrap();
        let no = tempfile::tempdir().unwrap();
        let (ok_p, no_p) = (ok.path().to_owned(), no.path().to_owned());
        std::thread::spawn(move || {
            let status = landlock(std::slice::from_ref(&ok_p)).unwrap();
            if status == "NotEnforced" {
                return; // kernel without Landlock
            }
            std::fs::write(ok_p.join("a"), "x").unwrap();
            let e = std::fs::write(no_p.join("a"), "x").unwrap_err();
            assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
            assert!(std::fs::read_dir(&no_p).is_ok());
        })
        .join()
        .unwrap();
    }
}
