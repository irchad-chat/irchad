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
4. **Bump** `version` in the root `Cargo.toml` and the `<releases>` entry in `packaging/linux/party.gentoo.IrChad.metainfo.xml`, then commit.
5. **Tag:** `git tag v0.2.0 && git push origin v0.2.0`. `.gitea/workflows/release.yml` builds everything into a draft release and publishes it once every job passes:
   - `irchad-<v>-x86_64.flatpak` (install: `flatpak install --user irchad-<v>-x86_64.flatpak`)
   - `irchad-<v>-linux-x86_64.tar.gz` (binary plus `install.sh`) and the bare `irchad-linux-x86_64`
   - `irchad-windows-x86_64-setup.exe` (per-user, no admin) and the bare `irchad-windows-x86_64.exe`
   - the image `git.gentoo.party/sam/chadd:<v>` and `:latest`

Runner prerequisites: the Linux act_runner must allow privileged job containers (flatpak-builder),
the Windows runner needs NSIS (or Chocolatey to install it), and the repo needs a `REGISTRY_TOKEN` secret with package write access.
