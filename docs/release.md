# Releasing

1. **Fuzz** every target for a while (an hour each, or overnight). Fuzzing stays out of CI.
   ```sh
   cargo install cargo-fuzz          # needs nightly: rustup toolchain install nightly
   cd crates/chad-irc/fuzz && cargo +nightly fuzz run message -- -max_total_time=3600
   ```
   Targets: `chad-irc/fuzz`: `message`. Add new targets to this list.
   A crash leaves its input in `fuzz/artifacts/`. Fix it and add the input as a unit test before going on.
2. **Live checks** against the dev stack (`deploy/README.md`):
   `cargo test -p chad-core --test live -- --ignored` and `cargo test -p chad-desktop shots -- --ignored`.
   Look over `target/shots/*.png`.
3. **By hand**, on GNOME Wayland, KDE Wayland and an X11 session, plus a Windows smoke test:
   join from an invite link in a browser, chat, upload by paste and by the paperclip, a call, a screen share with audio, notifications and the tray.
4. **Bump and tag:** `./scripts/release.sh v0.2.0` bumps `version` in the root `Cargo.toml` and the `<releases>` entry in `packaging/linux/party.gentoo.IrChad.metainfo.xml`, commits, tags and pushes.
5. **Build:** `.gitea/workflows/release.yml` builds everything into a draft release and publishes it once every job passes:
   - `irchad-linux-x86_64.tar.gz` (binary plus `install.sh`; the name is unversioned so `latest/` links work)
   - `irchad-windows-x86_64-setup.exe` (per-user, no admin)
   - the image `git.gentoo.party/sam/chadd:<v>` and `:latest`

The Flatpak job is disabled for now (`if: false`); re-enabling it needs the Linux act_runner to allow privileged job containers (flatpak-builder).
Runner prerequisites: the Windows runner needs NSIS (or Chocolatey to install it), the repo needs a `REGISTRY_TOKEN` secret with package write access, and the Linux job container `192.0.2.7:3000/sam/irchad-ci` must exist (`ci-image.yml`, D32).
