//! `mijia login`: QR-code login to the Xiaomi account. Only the account owner runs this,
//! in their own terminal. The CLI never asks for or handles the account password.
//!
//! The flow (as used by open-source tools such as Xiaomi-cloud-tokens-extractor, MIT):
//! ask account.xiaomi.com for a login QR code, long-poll until the user scans it in the
//! Mi Home app and confirms, then follow the returned `location` to obtain the
//! `serviceToken` cookie for the `xiaomiio` service.

use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::cloud::{self, Cloud};
use crate::config::{self, Paths, REGIONS};
use crate::error::{fail, Error, Result};

const LOGIN_URL: &str = "https://account.xiaomi.com/longPolling/loginUrl";
const RESPONSE_PREFIX: &str = "&&&START&&&";

/// account.xiaomi.com prefixes its JSON responses with `&&&START&&&`.
pub fn parse_account_response(body: &str) -> Result<Value> {
    let json = body.trim_start().strip_prefix(RESPONSE_PREFIX).unwrap_or(body);
    Ok(serde_json::from_str(json)?)
}

/// The value of cookie `name` from a `Set-Cookie` header, if that header sets it.
pub fn cookie_value(set_cookie: &str, name: &str) -> Option<String> {
    let (key, value) = set_cookie.split(';').next()?.split_once('=')?;
    (key.trim() == name).then(|| value.trim().trim_matches('"').to_string()).filter(|v| !v.is_empty())
}

fn random_device_id() -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS random number generator failed");
    bytes.iter().map(|b| CHARS[*b as usize % CHARS.len()] as char).collect()
}

/// Deletes the QR code image however the login ends.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn save_qr(png: &[u8]) -> Result<TempFile> {
    let mut suffix = [0u8; 6];
    getrandom::fill(&mut suffix).expect("the OS random number generator failed");
    let name: String = suffix.iter().map(|b| format!("{b:02x}")).collect();
    let path = std::env::temp_dir().join(format!("mijia-login-qr-{name}.png"));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    std::io::Write::write_all(&mut options.open(&path)?, png)?;
    Ok(TempFile(path))
}

fn open_image(path: &Path) -> bool {
    let status = if cfg!(target_os = "macos") {
        Command::new("open").arg(path).status()
    } else if cfg!(windows) {
        Command::new("cmd").args(["/C", "start", ""]).arg(path).status()
    } else {
        Command::new("xdg-open").arg(path).status()
    };
    status.is_ok_and(|s| s.success())
}

fn get_text(agent: &ureq::Agent, url: &str) -> Result<(u16, String)> {
    let mut response = agent.get(url).call()?;
    let status = response.status().as_u16();
    Ok((status, response.body_mut().read_to_string()?))
}

/// Follows redirects by hand so the `serviceToken` cookie is found on whichever hop sets it.
fn fetch_service_token(location: &str) -> Result<Option<String>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .build()
        .into();
    let mut url = location.to_string();
    for _ in 0..10 {
        let response = agent.get(&url).call()?;
        let token = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|h| h.to_str().ok())
            .find_map(|h| cookie_value(h, "serviceToken"));
        if token.is_some() {
            return Ok(token);
        }
        let next = response.headers().get("location").and_then(|h| h.to_str().ok()).map(str::to_string);
        match next {
            Some(next) if response.status().is_redirection() => {
                url = if next.starts_with("http") {
                    next
                } else {
                    // Relative redirect: keep scheme and host of the current URL.
                    let origin_end = url.match_indices('/').nth(2).map_or(url.len(), |(i, _)| i);
                    format!("{}{}", &url[..origin_end], next)
                };
            }
            _ => break,
        }
    }
    Ok(None)
}

fn qr_login(agent: &ureq::Agent) -> Result<Value> {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    // `qs` is deliberately percent-encoded twice, as the Mi Home login page sends it.
    let url = format!(
        "{LOGIN_URL}?_qrsize=480&qs=%253Fsid%253Dxiaomiio%2526_json%253Dtrue\
         &callback=https://sts.api.io.mi.com/sts&_hasLogo=false&sid=xiaomiio\
         &serviceParam=&_locale=zh_CN&_dc={millis}"
    );
    let (_, body) = get_text(agent, &url)?;
    let info = parse_account_response(&body)?;
    let (Some(qr_url), Some(poll_url)) = (info["qr"].as_str(), info["lp"].as_str()) else {
        return fail("Xiaomi did not return a login QR code; try again later");
    };
    let wait = info["timeout"].as_u64().unwrap_or(300);

    let mut response = agent.get(qr_url).call()?;
    let png = response.body_mut().read_to_vec()?;
    let qr = save_qr(&png)?;
    if open_image(&qr.0) {
        println!("Opened the login QR code ({}).", qr.0.display());
    } else {
        println!("Open this QR code image: {}", qr.0.display());
    }
    println!("No image viewer here (e.g. over SSH)? Open this link in any browser to show the QR code:\n  {qr_url}");
    println!("In the Mi Home app tap + (top right) → Scan, scan the code, then confirm the login on the phone.");
    println!("Waiting up to {wait} seconds...");

    let deadline = Instant::now() + Duration::from_secs(wait);
    let data = loop {
        if Instant::now() > deadline {
            return fail("the QR code was not scanned in time; run `mijia login` again");
        }
        let polled = agent.get(poll_url).config().timeout_global(Some(Duration::from_secs(10))).build().call();
        match polled {
            Ok(mut r) if r.status().as_u16() == 200 => break parse_account_response(&r.body_mut().read_to_string()?)?,
            // The long poll itself waited; ask again straight away.
            Err(ureq::Error::Timeout(_)) => {}
            _ => sleep(Duration::from_secs(1)),
        }
    };
    drop(qr);

    let field = |key: &str| data.get(key).filter(|v| v.is_string() || v.is_number()).cloned();
    let (Some(user_id), Some(pass_token), Some(ssecurity), Some(location)) =
        (field("userId"), field("passToken"), field("ssecurity"), data["location"].as_str())
    else {
        return fail("the login was not completed (QR code expired or login rejected); run `mijia login` again");
    };
    let Some(service_token) = fetch_service_token(location)? else {
        return fail("scanned, but Xiaomi did not return a service token; run `mijia login` again");
    };
    // Same layout as MiService's token file, so the cache works with either tool.
    Ok(json!({
        "deviceId": random_device_id(),
        "userId": user_id,
        "passToken": pass_token,
        "xiaomiio": [ssecurity, service_token],
    }))
}

fn count_devices(paths: &Paths, region: &str) -> Result<usize> {
    let cloud = Cloud::new(config::load_token(&paths.token)?, &paths.token, region)?;
    Ok(cloud::device_list(&cloud)?.len())
}

pub fn run(paths: &Paths, region_flag: Option<String>) -> Result<()> {
    // Agents run commands without a terminal. Refusing here keeps the QR link, the login
    // and the resulting credentials with the account owner, out of any agent transcript.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return fail(
            "`mijia login` must be run by the Xiaomi account owner in their own interactive terminal. \
             AI agents: do not run it; ask the user to run `mijia login` themselves.",
        );
    }
    let explicit_region = region_flag.is_some();
    let (region, _) = config::resolve_region(region_flag, paths)?;
    println!("Logging in to Xiaomi with a QR code. The login cache will be saved to {}", paths.token.display());
    println!("(readable only by you; it gives full access to the account, so never share it).\n");

    let token = qr_login(&cloud::http_agent())?;
    config::write_private(&paths.token, &(serde_json::to_string_pretty(&token)? + "\n"))?;
    if explicit_region {
        config::save_region(&paths.config, &region)?;
    }
    let count = count_devices(paths, &region).map_err(|e| match e {
        Error::LoginRequired(_) => {
            Error::Other("logged in, but the Xiaomi cloud rejected the new login; run `mijia login` again".into())
        }
        other => other,
    })?;
    println!("Logged in. {count} device(s) in region {region}.");
    if count > 0 || explicit_region {
        return Ok(());
    }
    // An empty list usually means the account lives in another region: look for it.
    println!("Checking the other regions...");
    let mut found = Vec::new();
    for other in REGIONS.iter().filter(|r| **r != region) {
        match count_devices(paths, other) {
            Ok(n) => {
                println!("  {other}: {n} device(s)");
                if n > 0 {
                    found.push(*other);
                }
            }
            Err(e) => println!("  {other}: {e}"),
        }
    }
    match found.as_slice() {
        [only] => {
            config::save_region(&paths.config, only)?;
            println!("Saved region {only} to {}.", paths.config.display());
        }
        [] => println!("No devices in any region. Add devices in the Mi Home app first."),
        _ => println!("Devices in several regions; choose one with: mijia login --region <region>"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_account_prefix() {
        let v = parse_account_response("&&&START&&&{\"code\":0,\"qr\":\"https://example/qr.png\"}").unwrap();
        assert_eq!(v["qr"], "https://example/qr.png");
        assert_eq!(parse_account_response("{\"a\":1}").unwrap()["a"], 1);
    }

    #[test]
    fn reads_cookies() {
        assert_eq!(
            cookie_value("serviceToken=abc/def==; Path=/; Domain=.api.io.mi.com", "serviceToken").as_deref(),
            Some("abc/def==")
        );
        assert_eq!(cookie_value("serviceToken=\"q\"; Path=/", "serviceToken").as_deref(), Some("q"));
        assert_eq!(cookie_value("userId=1; Path=/", "serviceToken"), None);
        assert_eq!(cookie_value("serviceToken=; Path=/", "serviceToken"), None);
    }

    #[test]
    fn device_id_shape() {
        let id = random_device_id();
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
    }

    #[test]
    fn refuses_without_a_terminal() {
        // `cargo test` runs without a terminal on stdin, the same situation as an agent's shell.
        if std::io::stdin().is_terminal() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("mijia-test-login-{}", std::process::id()));
        let paths = Paths { token: dir.join("token.json"), config: dir.join("config.json"), cache: dir.join("cache") };
        let err = run(&paths, None).unwrap_err();
        assert!(err.to_string().contains("account owner"));
        assert!(!paths.token.exists());
    }
}
