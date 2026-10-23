//! Where things live on disk, which cloud region to use, and reading the login cache.
//!
//! Only `mijia login` ever writes the token. Every other command opens it read-only and
//! never prints it, so an agent can use the CLI without seeing the credentials.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{fail, Error, Result};

pub const DEFAULT_REGION: &str = "cn";
/// Server regions of the Xiaomi cloud. The Mi Home app shows the user's region under
/// Profile → Settings → Region; mainland China is `cn`.
pub const REGIONS: &[&str] = &["cn", "de", "i2", "ru", "sg", "us", "tw", "in"];

pub struct Paths {
    pub token: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
}

impl Paths {
    pub fn resolve(token_flag: Option<PathBuf>) -> Result<Paths> {
        let config_dir = config_dir()?;
        Ok(Paths {
            token: token_flag.or_else(|| env_path("MIJIA_TOKEN")).unwrap_or_else(|| config_dir.join("token.json")),
            config: config_dir.join("config.json"),
            cache: cache_dir()?,
        })
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn home_dir() -> Result<PathBuf> {
    env_path("HOME")
        .or_else(|| env_path("USERPROFILE"))
        .ok_or_else(|| Error::Other("cannot find the home directory (HOME is not set)".into()))
}

fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = env_path("MIJIA_CONFIG_DIR") {
        return Ok(dir);
    }
    if cfg!(windows) {
        if let Some(dir) = env_path("APPDATA") {
            return Ok(dir.join("mijia"));
        }
    }
    // ~/.config on macOS too: it is where people look for a CLI's settings.
    Ok(env_path("XDG_CONFIG_HOME").map_or_else(|| home_dir().map(|h| h.join(".config")), Ok)?.join("mijia"))
}

fn cache_dir() -> Result<PathBuf> {
    if let Some(dir) = env_path("MIJIA_CACHE_DIR") {
        return Ok(dir);
    }
    if cfg!(windows) {
        if let Some(dir) = env_path("LOCALAPPDATA") {
            return Ok(dir.join("mijia").join("cache"));
        }
    }
    Ok(env_path("XDG_CACHE_HOME").map_or_else(|| home_dir().map(|h| h.join(".cache")), Ok)?.join("mijia"))
}

/// Region precedence: `--region`, then `MIJIA_REGION`, then what `mijia login` saved, then `cn`.
pub fn resolve_region(flag: Option<String>, paths: &Paths) -> Result<(String, &'static str)> {
    let (region, source) = if let Some(r) = flag {
        (r, "--region")
    } else if let Some(r) = env::var("MIJIA_REGION").ok().filter(|r| !r.is_empty()) {
        (r, "MIJIA_REGION")
    } else if let Some(r) = saved_region(&paths.config) {
        (r, "config.json")
    } else {
        (DEFAULT_REGION.to_string(), "default")
    };
    let region = region.to_lowercase();
    if !REGIONS.contains(&region.as_str()) {
        return fail(format!("unknown region {region:?}; use one of {}", REGIONS.join(", ")));
    }
    Ok((region, source))
}

fn saved_region(config: &Path) -> Option<String> {
    let text = fs::read_to_string(config).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    value.get("region")?.as_str().map(str::to_string)
}

pub fn save_region(config: &Path, region: &str) -> Result<()> {
    let mut value: Value = fs::read_to_string(config)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    value["region"] = Value::from(region);
    if let Some(dir) = config.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(config, serde_json::to_string_pretty(&value)? + "\n")?;
    Ok(())
}

/// The fields of the login cache that requests need. The file format is the one
/// MiService (`MiAccount`) uses, so existing `.mi.token` files work as they are.
pub struct Token {
    pub user_id: String,
    pub device_id: String,
    pub ssecurity: String,
    pub service_token: String,
}

impl std::fmt::Debug for Token {
    // Never let the secrets reach a log line or an error message.
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("Token { .. }")
    }
}

pub fn login_hint(path: &Path) -> String {
    format!(
        "Not logged in to Xiaomi, or the login has expired (login cache: {}). \
         The account owner must run `mijia login` in their own terminal. \
         AI agents: do not try to log in; stop and ask the user.",
        path.display()
    )
}

pub fn load_token(path: &Path) -> Result<Token> {
    let relogin = || Error::LoginRequired(login_hint(path));
    let text = fs::read_to_string(path).map_err(|_| relogin())?;
    warn_if_shared(path);
    // Parse into a Value first: typed deserialization errors can quote field contents.
    let value: Value = serde_json::from_str(&text).map_err(|_| relogin())?;
    let field = |key: &str| -> Option<String> {
        match value.get(key)? {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    };
    let pair = value.get("xiaomiio").and_then(Value::as_array).ok_or_else(relogin)?;
    let part = |i: usize| pair.get(i).and_then(Value::as_str).map(str::to_string);
    Ok(Token {
        user_id: field("userId").ok_or_else(relogin)?,
        device_id: field("deviceId").ok_or_else(relogin)?,
        ssecurity: part(0).ok_or_else(relogin)?,
        service_token: part(1).ok_or_else(relogin)?,
    })
}

#[cfg(unix)]
fn warn_if_shared(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        if meta.permissions().mode() & 0o077 != 0 {
            eprintln!("warning: {} can be read by other users; run: chmod 600 {}", path.display(), path.display());
        }
    }
}

#[cfg(not(unix))]
fn warn_if_shared(_path: &Path) {}

/// Write a file only the current user can read, replacing it atomically.
pub fn write_private(path: &Path, contents: &str) -> Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if !dir.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(dir)?;
    }
    let tmp = dir.join(format!(".{}.tmp", path.file_name().unwrap_or_default().to_string_lossy()));
    // A leftover from an interrupted run could have looser permissions, and opening an existing
    // file keeps them: always start from a new file so the mode below applies.
    let _ = fs::remove_file(&tmp);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    {
        use std::io::Write;
        let mut file = options.open(&tmp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("mijia-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_miservice_token_format() {
        let dir = temp_dir("token");
        let path = dir.join("token.json");
        write_private(
            &path,
            r#"{"deviceId": "ABCDEF", "userId": 12345, "passToken": "p", "xiaomiio": ["c2VjcmV0", "svc"]}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            // A stale, world-readable temp file must not leak its mode into the new token.
            use std::os::unix::fs::PermissionsExt;
            let stale = dir.join(".token.json.tmp");
            fs::write(&stale, "old").unwrap();
            fs::set_permissions(&stale, fs::Permissions::from_mode(0o644)).unwrap();
            write_private(&path, r#"{"deviceId": "ABCDEF", "userId": 12345, "xiaomiio": ["c2VjcmV0", "svc"]}"#)
                .unwrap();
            assert!(!stale.exists());
        }
        let token = load_token(&path).unwrap();
        assert_eq!(token.user_id, "12345");
        assert_eq!(token.device_id, "ABCDEF");
        assert_eq!(token.ssecurity, "c2VjcmV0");
        assert_eq!(token.service_token, "svc");
        assert_eq!(format!("{token:?}"), "Token { .. }");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn broken_or_missing_token_asks_for_login_without_echoing_it() {
        let dir = temp_dir("badtoken");
        let missing = load_token(&dir.join("nope.json")).unwrap_err();
        assert!(matches!(missing, Error::LoginRequired(_)));
        let path = dir.join("token.json");
        fs::write(&path, r#"{"userId": 1, "deviceId": "x", "xiaomiio": "SECRET-VALUE"}"#).unwrap();
        let err = load_token(&path).unwrap_err();
        assert!(matches!(err, Error::LoginRequired(_)));
        assert!(!err.to_string().contains("SECRET-VALUE"));
    }

    #[test]
    fn region_precedence() {
        let dir = temp_dir("region");
        let paths = Paths { token: dir.join("t"), config: dir.join("config.json"), cache: dir.join("c") };
        assert_eq!(resolve_region(Some("SG".into()), &paths).unwrap().0, "sg");
        assert!(resolve_region(Some("mars".into()), &paths).is_err());
        save_region(&paths.config, "de").unwrap();
        if env::var_os("MIJIA_REGION").is_none() {
            assert_eq!(resolve_region(None, &paths).unwrap(), ("de".to_string(), "config.json"));
        }
    }
}
