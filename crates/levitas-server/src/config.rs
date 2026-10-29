//! Server settings from `server.properties`, in vanilla's format and with vanilla's key
//! names, so an existing file keeps working. A missing file is written with the
//! defaults; unknown keys are kept as they are and ignored.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use tracing::{info, warn};

const FILE: &str = "server.properties";

#[derive(Debug, Clone)]
pub struct Config {
    pub server_ip: String,
    pub server_port: u16,
    pub motd: String,
    pub max_players: u32,
    pub view_distance: i32,
    /// Packets at least this long are compressed; negative turns compression off.
    pub compression_threshold: i32,
    pub white_list: bool,
    pub level_name: String,
    pub level_seed: Option<i64>,
    pub level_type: String,
    pub online_mode: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            // Localhost by default; set it to 0.0.0.0 (with online-mode=true) to open
            // the server to others.
            server_ip: "127.0.0.1".to_owned(),
            server_port: 25565,
            motd: "A Levitas Server".to_owned(),
            max_players: 20,
            view_distance: 8,
            // Test clients read packets uncompressed.
            compression_threshold: if cfg!(test) { -1 } else { 256 },
            white_list: false,
            level_name: "world".to_owned(),
            level_seed: None,
            level_type: "minecraft:normal".to_owned(),
            online_mode: false,
        }
    }
}

static CONFIG: OnceLock<Config> = OnceLock::new();

/// The settings in force: the file's once [`load`] ran, the defaults before (in tests).
pub fn get() -> &'static Config {
    CONFIG.get_or_init(Config::default)
}

/// Reads `server.properties` from `dir`, writing it with defaults if it is missing.
pub fn load(dir: &Path) -> &'static Config {
    let path = dir.join(FILE);
    let text = fs::read_to_string(&path).unwrap_or_default();
    let mut values = parse(&text);
    let defaults = Config::default();
    let mut changed = false;
    let mut get_or = |key: &str, default: String| -> String {
        values
            .entry(key.to_owned())
            .or_insert_with(|| {
                changed = true;
                default
            })
            .clone()
    };
    let number = |key: &str, text: String, default: i64| -> i64 {
        text.trim().parse().unwrap_or_else(|_| {
            warn!("{FILE}: {key}={text} is not a number, using {default}");
            default
        })
    };
    let ip = get_or("server-ip", defaults.server_ip.clone());
    let port = get_or("server-port", defaults.server_port.to_string());
    let motd = get_or("motd", defaults.motd.clone());
    let max_players = get_or("max-players", defaults.max_players.to_string());
    let view = get_or("view-distance", defaults.view_distance.to_string());
    let compression = get_or(
        "network-compression-threshold",
        defaults.compression_threshold.to_string(),
    );
    let white_list = get_or("white-list", "false".to_owned());
    let level_name = get_or("level-name", defaults.level_name.clone());
    let seed = get_or("level-seed", String::new());
    let level_type = get_or("level-type", defaults.level_type.clone());
    let online = get_or("online-mode", "false".to_owned());
    let config = Config {
        server_ip: ip,
        server_port: number("server-port", port, 25565).clamp(1, 65535) as u16,
        motd,
        max_players: number("max-players", max_players, 20).max(1) as u32,
        view_distance: number("view-distance", view, 8).clamp(2, 32) as i32,
        compression_threshold: number("network-compression-threshold", compression, 256) as i32,
        white_list: white_list.trim() == "true",
        level_name,
        level_seed: seed_from(&seed),
        level_type,
        online_mode: online.trim() == "true",
    };
    if changed {
        if let Err(e) = fs::write(&path, write(&values)) {
            warn!("could not write {}: {e}", path.display());
        } else {
            info!("wrote {} with default settings", path.display());
        }
    }
    CONFIG.get_or_init(|| config)
}

/// A seed as vanilla reads it: a number, or any other text hashed like Java's
/// `String.hashCode`; empty for a random one.
fn seed_from(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(text.parse().unwrap_or_else(|_| {
        let hash = text.encode_utf16().fold(0i32, |h, c| {
            h.wrapping_mul(31).wrapping_add(i32::from(c as i16))
        });
        i64::from(hash)
    }))
}

fn parse(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!'))
        .filter_map(|l| {
            let (key, value) = l.split_once('=').or_else(|| l.split_once(':'))?;
            Some((key.trim().to_owned(), value.trim().to_owned()))
        })
        .collect()
}

fn write(values: &BTreeMap<String, String>) -> String {
    let mut out = String::from("#Minecraft server properties (Levitas)\n");
    for (key, value) in values {
        out.push_str(&format!("{key}={value}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_keep_unknown_keys_and_read_vanilla_seeds() {
        let values = parse("# comment\nmotd=Hello\nspawn-protection=16\n");
        assert_eq!(values["motd"], "Hello");
        assert_eq!(values["spawn-protection"], "16");
        assert_eq!(seed_from("12345"), Some(12345));
        // Java's "hello".hashCode().
        assert_eq!(seed_from("hello"), Some(99_162_322));
        assert_eq!(seed_from(""), None);
    }
}
