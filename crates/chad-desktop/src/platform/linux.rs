use std::collections::HashMap;
use std::path::PathBuf;

use ksni::TrayMethods as _;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::zvariant::OwnedValue;

use super::{APP_ID, Os, OsTx};

const PATH: &str = "/party/gentoo/IrChad";

/// Holds the bus name while the app runs.
pub struct Instance(#[allow(dead_code)] Option<zbus::Connection>);

/// `org.freedesktop.Application`, so a second launch (or the desktop) can hand
/// us a URL or raise the window.
struct Application(OsTx);

#[zbus::interface(name = "org.freedesktop.Application")]
impl Application {
    fn activate(&self, _platform_data: HashMap<String, OwnedValue>) {
        self.0.send(Os::Show);
    }

    fn open(&self, uris: Vec<String>, _platform_data: HashMap<String, OwnedValue>) {
        for uri in uris.into_iter().take(8) {
            self.0.send(Os::Open(uri));
        }
        self.0.send(Os::Show);
    }

    fn activate_action(
        &self,
        _action_name: String,
        _parameter: Vec<OwnedValue>,
        _platform_data: HashMap<String, OwnedValue>,
    ) {
    }
}

#[zbus::proxy(
    interface = "org.freedesktop.Application",
    default_service = "party.gentoo.IrChad",
    default_path = "/party/gentoo/IrChad"
)]
trait Running {
    fn activate(&self, platform_data: HashMap<String, OwnedValue>) -> zbus::Result<()>;
    fn open(
        &self,
        uris: Vec<String>,
        platform_data: HashMap<String, OwnedValue>,
    ) -> zbus::Result<()>;
}

/// `None` when another instance is running and got `url`: exit.
/// Without a session bus every launch is its own instance.
pub async fn single_instance(url: Option<String>, tx: OsTx) -> Option<Instance> {
    let conn = match zbus::connection::Builder::session()
        .and_then(|b| b.serve_at(PATH, Application(tx)))
    {
        Ok(b) => b.build().await,
        Err(e) => Err(e),
    };
    let conn = match conn {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("no D-Bus session bus, single instance off: {e}");
            return Some(Instance(None));
        }
    };
    match conn
        .request_name_with_flags(APP_ID, RequestNameFlags::DoNotQueue.into())
        .await
    {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
            Some(Instance(Some(conn)))
        }
        _ => {
            let r = match RunningProxy::new(&conn).await {
                Ok(p) => match url {
                    Some(u) => p.open(vec![u], HashMap::new()).await,
                    None => p.activate(HashMap::new()).await,
                },
                Err(e) => Err(e),
            };
            match r {
                Ok(()) => None,
                Err(e) => {
                    tracing::warn!("couldn't reach the running IrChad: {e}");
                    Some(Instance(None))
                }
            }
        }
    }
}

/// Installs a user `.desktop` file that handles `x-scheme-handler/irchad`,
/// unless a packaged one exists (or we're in a Flatpak, whose manifest does it).
pub fn register_scheme() {
    if std::env::var_os("FLATPAK_ID").is_some() {
        return;
    }
    let file = format!("{APP_ID}.desktop");
    let data_dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    if data_dirs
        .split(':')
        .any(|d| PathBuf::from(d).join("applications").join(&file).exists())
    {
        return;
    }
    let Some(dir) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".local/share")))
        .map(|d| d.join("applications"))
    else {
        return;
    };
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let entry = desktop_entry(&exe.to_string_lossy());
    let icon = dir.join(format!("../icons/hicolor/scalable/apps/{APP_ID}.svg"));
    let svg = include_str!("../../../../packaging/linux/party.gentoo.IrChad.svg");
    if std::fs::read_to_string(&icon).ok().as_deref() != Some(svg) {
        let r = icon.parent().map_or(Ok(()), std::fs::create_dir_all);
        if let Err(e) = r.and_then(|()| std::fs::write(&icon, svg)) {
            tracing::warn!("installing {}: {e}", icon.display());
        }
    }
    let path = dir.join(&file);
    if std::fs::read_to_string(&path).is_ok_and(|s| s == entry) {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, entry)) {
        tracing::warn!("installing {}: {e}", path.display());
        return;
    }
    // Best effort: these tools may not be installed.
    for cmd in [
        vec!["xdg-mime", "default", &file, "x-scheme-handler/irchad"],
        vec!["update-desktop-database", &dir.to_string_lossy()],
    ] {
        let _ = std::process::Command::new(cmd[0]).args(&cmd[1..]).status();
    }
}

fn desktop_entry(exe: &str) -> String {
    // Desktop Entry spec: quote the path, escape `"`, `` ` ``, `$` and `\`.
    let mut quoted = String::from('"');
    for c in exe.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    format!(
        "[Desktop Entry]\nType=Application\nName=IrChad\nComment=Chat with your friends\n\
         Exec={quoted} %u\nIcon={APP_ID}\nTerminal=false\n\
         Categories=Network;Chat;InstantMessaging;\nMimeType=x-scheme-handler/irchad;\n\
         StartupWMClass={APP_ID}\n"
    )
}

/// StatusNotifierItem tray. Missing on desktops without a tray (stock GNOME).
pub struct Tray(Option<ksni::Handle<Sni>>);

struct Sni {
    tx: OsTx,
    unread: bool,
    mentions: u32,
}

impl ksni::Tray for Sni {
    fn id(&self) -> String {
        APP_ID.into()
    }
    fn title(&self) -> String {
        super::tray_title(self.mentions)
    }
    fn status(&self) -> ksni::Status {
        if self.mentions > 0 {
            ksni::Status::NeedsAttention
        } else {
            ksni::Status::Active
        }
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        [22, 32, 64]
            .into_iter()
            .map(|s| ksni::Icon {
                width: s as i32,
                height: s as i32,
                data: super::tray_icon(s, self.unread, self.mentions > 0),
            })
            .collect()
    }
    fn attention_icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon_pixmap()
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        self.tx.send(Os::Show);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem {
                label: "Show IrChad".into(),
                activate: Box::new(|t: &mut Self| t.tx.send(Os::Show)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|t: &mut Self| t.tx.send(Os::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

impl Tray {
    pub async fn spawn(tx: OsTx) -> Self {
        let sni = Sni {
            tx,
            unread: false,
            mentions: 0,
        };
        // Flatpak can't own `StatusNotifierItem-<pid>-<n>`; watchers take the unique name.
        let sandboxed = std::env::var_os("FLATPAK_ID").is_some();
        match sni.disable_dbus_name(sandboxed).spawn().await {
            Ok(h) => Self(Some(h)),
            Err(e) => {
                tracing::info!("no tray: {e}");
                Self(None)
            }
        }
    }

    /// No icon, so tests don't put one in the real tray.
    #[cfg(test)]
    pub fn none() -> Self {
        Self(None)
    }

    pub fn set(&self, unread: bool, mentions: u32) {
        if let Some(h) = self.0.clone() {
            tokio::spawn(async move {
                h.update(|t| {
                    t.unread = unread;
                    t.mentions = mentions;
                })
                .await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn desktop_entry_quotes_exec() {
        let e = super::desktop_entry("/opt/my apps/ir$chad");
        assert!(e.contains("Exec=\"/opt/my apps/ir\\$chad\" %u\n"));
    }
}
