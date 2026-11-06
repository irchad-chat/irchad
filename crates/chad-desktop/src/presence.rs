//! What we share as `chad/activity` (D63): a game's own Rich Presence, else a
//! game we see running, else what a media player is playing.

use std::collections::HashMap;

use chad_core::Game;

use crate::icons;

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

/// The line to share, best source first; empty for nothing.
pub fn line(rich: Option<String>, game: Option<&str>, music: Option<&str>) -> String {
    rich.or_else(|| game.map(|g| format!("Playing {g}")))
        .or_else(|| music.map(|m| format!("Listening to {m}")))
        .unwrap_or_default()
}

/// The icon next to an activity line.
pub fn icon(line: &str) -> &'static str {
    if line.starts_with("Listening to") {
        icons::MUSIC_NOTES
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
    fn rich_presence_beats_a_running_game_beats_music() {
        let rich = Some("Playing Celeste: Chapter 3".to_owned());
        assert_eq!(
            line(rich, Some("X"), Some("Y")),
            "Playing Celeste: Chapter 3"
        );
        assert_eq!(line(None, Some("X"), Some("Y")), "Playing X");
        assert_eq!(line(None, None, Some("Y by Z")), "Listening to Y by Z");
        assert_eq!(line(None, None, None), "");
        assert_eq!(icon("Listening to Y"), icons::MUSIC_NOTES);
    }
}
