//! Finding CS2 and managing the one file freezetime puts in its folder:
//! `game/csgo/cfg/gamestate_integration_freezetime.cfg`. CS2 reads it at launch
//! and starts sending match updates to the address inside. Nothing else in the
//! game's folder is read or written.

use crate::vdf;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const CFG_NAME: &str = "gamestate_integration_freezetime.cfg";
const APP_ID: &str = "730";
const DEFAULT_INSTALL_DIR: &str = "Counter-Strike Global Offensive";

/// Where Steam itself might be. The first one that exists wins.
pub fn steam_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        use winreg::RegKey;
        let read = |root, path: &str, name: &str| -> Option<String> {
            RegKey::predef(root).open_subkey(path).ok()?.get_value::<String, _>(name).ok().filter(|s| !s.trim().is_empty())
        };
        if let Some(p) = read(HKEY_CURRENT_USER, r"Software\Valve\Steam", "SteamPath") {
            out.push(PathBuf::from(p.replace('/', "\\")));
        }
        for key in [r"SOFTWARE\WOW6432Node\Valve\Steam", r"SOFTWARE\Valve\Steam"] {
            if let Some(p) = read(HKEY_LOCAL_MACHINE, key, "InstallPath") {
                out.push(PathBuf::from(p));
            }
        }
        out.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
        out.push(PathBuf::from(r"C:\Program Files\Steam"));
    }
    #[cfg(not(windows))]
    {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            for rel in [
                ".steam/steam",
                ".local/share/Steam",
                ".steam/debian-installation",
                ".var/app/com.valvesoftware.Steam/.local/share/Steam",
                "snap/steam/common/.local/share/Steam",
                "Library/Application Support/Steam",
            ] {
                out.push(home.join(rel));
            }
        }
    }
    out
}

/// Every Steam library folder listed in `libraryfolders.vdf`, the ones that
/// say they contain CS2 first.
pub fn libraries(steam: &Path) -> Vec<PathBuf> {
    let mut with_cs2 = Vec::new();
    let mut others = vec![steam.to_path_buf()];
    for file in [steam.join("steamapps/libraryfolders.vdf"), steam.join("config/libraryfolders.vdf")] {
        let Ok(text) = fs::read_to_string(&file) else { continue };
        let Ok(root) = vdf::parse(&text) else { continue };
        let Some(folders) = root.get_obj("libraryfolders") else { continue };
        for (_, entry) in &folders.entries {
            let vdf::Value::Obj(entry) = entry else { continue };
            let Some(path) = entry.get_str("path") else { continue };
            let path = PathBuf::from(vdf::unescape(path));
            if entry.get_obj("apps").is_some_and(|apps| apps.get(APP_ID).is_some()) {
                with_cs2.push(path);
            } else {
                others.push(path);
            }
        }
        break;
    }
    with_cs2.extend(others);
    with_cs2.dedup();
    with_cs2
}

/// CS2's cfg folder inside one Steam library, if CS2 is installed there.
pub fn cfg_dir_in_library(lib: &Path) -> Option<PathBuf> {
    let steamapps = lib.join("steamapps");
    let manifest = fs::read_to_string(steamapps.join(format!("appmanifest_{APP_ID}.acf"))).ok()?;
    let installdir = vdf::parse(&manifest)
        .ok()
        .and_then(|root| root.get_obj("AppState").and_then(|a| a.get_str("installdir")).map(vdf::unescape))
        .unwrap_or_else(|| DEFAULT_INSTALL_DIR.to_string());
    let cfg = steamapps.join("common").join(installdir).join("game").join("csgo").join("cfg");
    cfg.is_dir().then_some(cfg)
}

/// Finds CS2's cfg folder through Steam's library list.
pub fn find_cfg_dir(steam_dirs: &[PathBuf]) -> Option<PathBuf> {
    steam_dirs
        .iter()
        .filter(|d| d.join("steamapps").is_dir())
        .flat_map(|d| libraries(d))
        .find_map(|lib| cfg_dir_in_library(&lib))
}

/// Accepts a folder the user picked: the CS2 folder itself, `game/csgo`, or the cfg folder.
pub fn cfg_dir_from_pick(picked: &Path) -> Option<PathBuf> {
    [picked.join("game/csgo/cfg"), picked.join("csgo/cfg"), picked.join("cfg"), picked.to_path_buf()]
        .into_iter()
        .find(|p| p.ends_with("csgo/cfg") && p.is_dir())
}

pub fn cfg_text(port: u16, token: &str) -> String {
    format!(
        r#""freezetime"
{{
	"uri"		"http://127.0.0.1:{port}/"
	"timeout"	"1.1"
	"buffer"	"0.1"
	"throttle"	"0.25"
	"heartbeat"	"10.0"
	"auth"
	{{
		"token"	"{token}"
	}}
	"data"
	{{
		"provider"		"1"
		"map"			"1"
		"round"			"1"
		"player_id"		"1"
		"player_state"		"1"
		"player_match_stats"	"1"
	}}
}}
"#
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CfgStatus {
    /// Not there yet.
    Missing,
    /// There, but for another port or token (CS2 needs a restart after it's rewritten).
    Outdated,
    Installed,
}

pub fn status(cfg_dir: &Path, port: u16, token: &str) -> CfgStatus {
    match fs::read_to_string(cfg_dir.join(CFG_NAME)) {
        Ok(text) if text == cfg_text(port, token) => CfgStatus::Installed,
        Ok(_) => CfgStatus::Outdated,
        Err(_) => CfgStatus::Missing,
    }
}

/// Writes the cfg in one step, so CS2 never reads half a file.
pub fn install(cfg_dir: &Path, port: u16, token: &str) -> io::Result<PathBuf> {
    let path = cfg_dir.join(CFG_NAME);
    let tmp = cfg_dir.join(format!("{CFG_NAME}.tmp"));
    fs::write(&tmp, cfg_text(port, token))?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

pub fn remove(cfg_dir: &Path) -> io::Result<bool> {
    match fs::remove_file(cfg_dir.join(CFG_NAME)) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("freezetime-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A Steam install whose CS2 lives in a second library, the way it often does
    /// on a PC with the game on another drive.
    fn fake_steam(root: &Path) -> (PathBuf, PathBuf) {
        let steam = root.join("Steam");
        let games = root.join("Games");
        fs::create_dir_all(steam.join("steamapps")).unwrap();
        let games_vdf = games.display().to_string().replace('\\', "\\\\");
        let steam_vdf = steam.display().to_string().replace('\\', "\\\\");
        fs::write(
            steam.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{steam_vdf}\"\n\t\t\"apps\"\n\t\t{{\n\t\t\t\"228980\"\t\t\"1\"\n\t\t}}\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{games_vdf}\"\n\t\t\"label\"\t\t\"\"\n\t\t\"apps\"\n\t\t{{\n\t\t\t\"730\"\t\t\"38163923714\"\n\t\t}}\n\t}}\n}}\n"
            ),
        )
        .unwrap();
        fs::create_dir_all(games.join("steamapps")).unwrap();
        fs::write(
            games.join("steamapps/appmanifest_730.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"730\"\n\t\"name\"\t\t\"Counter-Strike 2\"\n\t\"installdir\"\t\t\"Counter-Strike Global Offensive\"\n}\n",
        )
        .unwrap();
        let cfg = games.join("steamapps/common/Counter-Strike Global Offensive/game/csgo/cfg");
        fs::create_dir_all(&cfg).unwrap();
        (steam, cfg)
    }

    #[test]
    fn finds_cs2_in_another_library() {
        let root = temp("find");
        let (steam, cfg) = fake_steam(&root);
        assert_eq!(find_cfg_dir(&[root.join("nope"), steam]), Some(cfg));
    }

    #[test]
    fn no_cs2_means_none() {
        let root = temp("none");
        fs::create_dir_all(root.join("Steam/steamapps")).unwrap();
        assert_eq!(find_cfg_dir(&[root.join("Steam")]), None);
    }

    #[test]
    fn install_status_and_remove() {
        let root = temp("install");
        let (_, cfg) = fake_steam(&root);
        assert_eq!(status(&cfg, 38127, "abc"), CfgStatus::Missing);
        install(&cfg, 38127, "abc").unwrap();
        assert_eq!(status(&cfg, 38127, "abc"), CfgStatus::Installed);
        assert_eq!(status(&cfg, 38127, "other"), CfgStatus::Outdated);
        let text = fs::read_to_string(cfg.join(CFG_NAME)).unwrap();
        let parsed = vdf::parse(&text).unwrap();
        let root_obj = parsed.get_obj("freezetime").unwrap();
        assert_eq!(root_obj.get_str("uri"), Some("http://127.0.0.1:38127/"));
        assert_eq!(root_obj.get_obj("auth").unwrap().get_str("token"), Some("abc"));
        assert!(remove(&cfg).unwrap());
        assert!(!remove(&cfg).unwrap());
        // Only our file was ever there.
        assert_eq!(fs::read_dir(&cfg).unwrap().count(), 0);
    }

    #[test]
    fn picked_folder_variants() {
        let root = temp("pick");
        let (_, cfg) = fake_steam(&root);
        let game_root = cfg.parent().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
        assert_eq!(cfg_dir_from_pick(&game_root), Some(cfg.clone()));
        assert_eq!(cfg_dir_from_pick(&cfg), Some(cfg.clone()));
        assert_eq!(cfg_dir_from_pick(&root), None);
    }
}
