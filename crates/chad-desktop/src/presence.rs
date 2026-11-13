//! What we share as `chad/activity` (D63): a game's own Rich Presence, else a
//! game we see running, else what a media player is playing.

use std::collections::HashMap;

use chad_core::{Game, Music};

use crate::icons;
use crate::platform::presence::Track;

/// Discord's game list from chadd, indexed for Rich Presence ids and for
/// running executables. Each name is kept once; the maps point into `names`.
pub struct Games {
    names: Vec<String>,
    ids: HashMap<String, u32>,
    /// Executable file name → (whole path suffix, game).
    by_exe: HashMap<String, Vec<(String, u32)>>,
}

impl Games {
    pub fn new(list: Vec<Game>) -> Self {
        let mut games = Self {
            names: Vec::with_capacity(list.len()),
            ids: HashMap::with_capacity(list.len()),
            by_exe: HashMap::new(),
        };
        for g in list {
            let Ok(i) = u32::try_from(games.names.len()) else {
                break;
            };
            for e in g.executables {
                let file = e.rsplit('/').next().unwrap_or(&e).to_owned();
                games.by_exe.entry(file).or_default().push((e, i));
            }
            games.ids.insert(g.id, i);
            games.names.push(g.name);
        }
        games
    }

    pub fn name(&self, id: &str) -> Option<&str> {
        self.ids.get(id).map(|i| self.names[*i as usize].as_str())
    }

    /// The first game whose executable is one of `paths`: full paths of running
    /// programs, Windows ones under Wine or Proton included.
    pub fn running(&self, paths: &[String]) -> Option<&str> {
        paths.iter().find_map(|p| {
            let p = p.replace('\\', "/").to_lowercase();
            // Wine's own programs share names with some games' (explorer.exe).
            if p.starts_with("c:/windows/") {
                return None;
            }
            let file = p.rsplit('/').next()?;
            self.by_exe.get(file)?.iter().find_map(|(suffix, i)| {
                let hit = p == *suffix
                    || p.strip_suffix(suffix.as_str())
                        .is_some_and(|rest| rest.ends_with('/'));
                hit.then(|| self.names[*i as usize].as_str())
            })
        })
    }
}

/// The first of `paths` whose file name is one of `custom` (executable names
/// the user added, compared ignoring case), as that name.
pub fn custom_game(paths: &[String], custom: &[String]) -> Option<String> {
    paths.iter().find_map(|p| {
        let file = p.rsplit(['/', '\\']).next()?;
        custom
            .iter()
            .find(|c| !c.trim().is_empty() && c.trim().eq_ignore_ascii_case(file))
            .map(|c| c.trim().to_owned())
    })
}

/// The line to share, best source first (empty for nothing), and the track
/// behind it when it's the media player's. `on` adds the player's name.
pub fn line(
    rich: Option<String>,
    game: Option<&str>,
    music: Option<&Track>,
    on: bool,
) -> (String, Option<Music>) {
    if let Some(l) = rich.or_else(|| game.map(|g| format!("Playing {g}"))) {
        return (l, None);
    }
    let Some(t) = music else {
        return (String::new(), None);
    };
    let verb = if t.video { "Watching" } else { "Listening to" };
    let line = match t.player.as_deref().filter(|_| on) {
        Some(p) => format!("{verb} {} on {p}", t.line),
        None => format!("{verb} {}", t.line),
    };
    (line, Some(t.music.clone()))
}

/// An activity line in full, as on a profile card: a link when its track has
/// one, and the track's progress under it.
pub fn show(ui: &mut egui::Ui, line: &str, music: Option<&Music>) {
    let text = format!("{} {line}", icon(line));
    match music.and_then(|m| m.url.as_ref()) {
        Some(u) => {
            ui.hyperlink_to(text, u.as_str()).on_hover_text(u.as_str());
        }
        None => {
            ui.label(text);
        }
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if let Some((at, len)) = music.and_then(|m| m.progress(now)) {
        ui.horizontal(|ui| {
            let clock = |s| {
                egui::RichText::new(chad_core::music::clock(s))
                    .small()
                    .weak()
            };
            ui.label(clock(at));
            let bar = egui::ProgressBar::new(at as f32 / len as f32).desired_height(4.0);
            ui.add_sized([120.0, 4.0], bar);
            ui.label(clock(len));
        });
        if at < len {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
}

/// The icon next to an activity line.
pub fn icon(line: &str) -> &'static str {
    if line.starts_with("Listening to") {
        icons::MUSIC_NOTES
    } else if line.starts_with("Watching") {
        icons::PLAY
    } else {
        icons::GAME_CONTROLLER
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn games() -> Games {
        Games::new(vec![
            Game {
                id: "1".into(),
                name: "DARK SOULS III".into(),
                executables: vec!["darksoulsiii.exe".into(), "game/darksoulsiii.exe".into()],
            },
            Game {
                id: "2".into(),
                name: "Shell Game".into(),
                executables: vec!["bin/explorer.exe".into(), "explorer.exe".into()],
            },
        ])
    }

    #[test]
    fn finds_games_by_executable() {
        let g = games();
        let run = |p: &[&str]| {
            let p: Vec<String> = p.iter().map(|s| (*s).to_owned()).collect();
            g.running(&p).map(str::to_owned)
        };
        let proton = r"Z:\home\me\.steam\steamapps\common\DARK SOULS III\Game\DarkSoulsIII.exe";
        assert_eq!(
            run(&["/usr/bin/bash", proton]).as_deref(),
            Some("DARK SOULS III")
        );
        assert_eq!(
            run(&["DarkSoulsIII.exe"]).as_deref(),
            Some("DARK SOULS III")
        );
        assert_eq!(run(&["/opt/notdarksoulsiii.exe"]), None);
        assert_eq!(run(&[r"C:\windows\explorer.exe"]), None);
        assert_eq!(g.name("1"), Some("DARK SOULS III"));
    }

    #[test]
    fn finds_custom_games_by_file_name() {
        let custom = vec!["MyGame.exe ".to_owned(), String::new()];
        let p = |s: &str| vec![s.to_owned()];
        assert_eq!(
            custom_game(&p(r"D:\Games\mygame.exe"), &custom).as_deref(),
            Some("MyGame.exe")
        );
        assert_eq!(
            custom_game(&p("/opt/x/mygame.exe"), &custom).as_deref(),
            Some("MyGame.exe")
        );
        assert_eq!(custom_game(&p("/opt/x/notmygame.exe"), &custom), None);
        assert_eq!(custom_game(&p("/usr/bin/"), &custom), None);
    }

    #[test]
    fn rich_presence_beats_a_running_game_beats_music() {
        let y = Track {
            line: "Y by Z".into(),
            player: Some("Spotify".into()),
            video: false,
            music: Music::parse("1 2").unwrap(),
        };
        let l =
            |rich: Option<&str>, game, music, on| line(rich.map(str::to_owned), game, music, on);
        let rich = "Playing Celeste: Chapter 3";
        assert_eq!(
            l(Some(rich), Some("X"), Some(&y), true),
            (rich.into(), None)
        );
        assert_eq!(l(None, Some("X"), Some(&y), true).0, "Playing X");
        assert_eq!(
            l(None, None, Some(&y), false),
            ("Listening to Y by Z".into(), Some(y.music.clone()))
        );
        assert_eq!(
            l(None, None, Some(&y), true).0,
            "Listening to Y by Z on Spotify"
        );
        let w = Track {
            video: true,
            ..y.clone()
        };
        assert_eq!(l(None, None, Some(&w), false).0, "Watching Y by Z");
        assert_eq!(l(None, None, None, true), (String::new(), None));
        assert_eq!(icon("Listening to Y"), icons::MUSIC_NOTES);
        assert_eq!(icon("Watching Y"), icons::PLAY);
    }
}
