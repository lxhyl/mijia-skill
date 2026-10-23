//! Xiaomi cloud client: request signing and the MiOT calls the CLI needs.
//!
//! Requests are form POSTs to `https://<region.>api.io.mi.com/app<uri>` carrying `_nonce`,
//! `data` (JSON) and `signature`; the account is identified by the `userId`, `serviceToken`
//! and `PassportDeviceId` cookies. The signing key is SHA-256(ssecurity ‖ nonce) and the
//! signature is HMAC-SHA256 over `<uri>&<signed nonce>&<nonce>&data=<data>`. This follows
//! what open-source clients such as MiService (MIT) do; the code here is written from scratch.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use hmac::{Hmac, Mac};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::{login_hint, Token};
use crate::error::{fail, Error, Result};

/// The cloud rejects reads of too many properties in one request.
const READ_BATCH: usize = 15;
const USER_AGENT: &str = "Android-7.1.1-1.0.0-ONEPLUS A3010-136-AIMIJIASKILL APP/xiaomi.smarthome APPV/62830";

/// One signed request to the Xiaomi cloud. A trait so the commands can be tested offline.
pub trait Api {
    /// POST `data` to `uri` (e.g. `/home/device_list`) and return the response's `result`.
    fn call(&self, uri: &str, data: &Value) -> Result<Value>;
}

pub fn server(region: &str) -> String {
    if region == "cn" {
        "https://api.io.mi.com/app".to_string()
    } else {
        format!("https://{region}.api.io.mi.com/app")
    }
}

pub fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .into()
}

/// 8 random bytes followed by the current minute since the epoch, big-endian.
pub fn new_nonce() -> [u8; 12] {
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce[..8]).expect("the OS random number generator failed");
    let minutes = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() / 60;
    nonce[8..].copy_from_slice(&(minutes as u32).to_be_bytes());
    nonce
}

/// Returns the base64 `_nonce` and `signature` form fields for a request.
pub fn sign(uri: &str, data: &str, ssecurity: &[u8], nonce: &[u8]) -> (String, String) {
    let nonce_b64 = B64.encode(nonce);
    let key = Sha256::new().chain_update(ssecurity).chain_update(nonce).finalize();
    let signed_nonce = B64.encode(key);
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).expect("HMAC accepts keys of any length");
    mac.update(format!("{uri}&{signed_nonce}&{nonce_b64}&data={data}").as_bytes());
    (nonce_b64, B64.encode(mac.finalize().into_bytes()))
}

pub struct Cloud {
    agent: ureq::Agent,
    server: String,
    token: Token,
    ssecurity: Vec<u8>,
    relogin: String,
}

impl Cloud {
    pub fn new(token: Token, token_path: &Path, region: &str) -> Result<Cloud> {
        let relogin = login_hint(token_path);
        let ssecurity = B64.decode(&token.ssecurity).map_err(|_| Error::LoginRequired(relogin.clone()))?;
        Ok(Cloud { agent: http_agent(), server: server(region), token, ssecurity, relogin })
    }
}

/// Writes go out on a fresh connection and are never retried here: only the caller knows
/// whether sending one twice is safe.
fn is_write(uri: &str) -> bool {
    matches!(uri, "/miotspec/prop/set" | "/miotspec/action")
}

/// The connection broke before a full response arrived. The cloud closes idle keep-alive
/// connections, so a request sent on a pooled one can fail with "unexpected end of file".
fn connection_dropped(e: &ureq::Error) -> bool {
    matches!(e, ureq::Error::Io(_) | ureq::Error::ConnectionFailed)
}

impl Cloud {
    fn send(&self, agent: &ureq::Agent, uri: &str, data: &str) -> std::result::Result<(u16, String), ureq::Error> {
        let (nonce, signature) = sign(uri, data, &self.ssecurity, &new_nonce());
        let cookie = format!(
            "userId={}; serviceToken={}; PassportDeviceId={}",
            self.token.user_id, self.token.service_token, self.token.device_id
        );
        let mut response = agent
            .post(format!("{}{uri}", self.server))
            .header("User-Agent", USER_AGENT)
            .header("x-xiaomi-protocal-flag-cli", "PROTOCAL-HTTP2")
            .header("Cookie", cookie)
            .send_form([("_nonce", nonce.as_str()), ("data", data), ("signature", signature.as_str())])?;
        let status = response.status().as_u16();
        Ok((status, response.body_mut().read_to_string()?))
    }
}

impl Api for Cloud {
    fn call(&self, uri: &str, data: &Value) -> Result<Value> {
        let data = data.to_string();
        let write = is_write(uri);
        let mut attempts = 0;
        let (status, body) = loop {
            attempts += 1;
            let agent = if write { http_agent() } else { self.agent.clone() };
            match self.send(&agent, uri, &data) {
                Ok(reply) => break reply,
                // A read is safe to repeat; one retry gets a new connection.
                Err(e) if !write && attempts == 1 && connection_dropped(&e) => continue,
                Err(e @ (ureq::Error::Io(_) | ureq::Error::Timeout(_))) if write => {
                    return Err(Error::Interrupted(format!(
                        "the connection to the Xiaomi cloud broke ({e}) after the command was sent, so it may \
                         or may not have reached the device. Check with `mijia status` before trying again"
                    )))
                }
                Err(e) => return fail(format!("cannot reach the Xiaomi cloud: {e}")),
            }
        };
        match status {
            200 => {}
            401 => return Err(Error::LoginRequired(self.relogin.clone())),
            status => return fail(format!("the Xiaomi cloud returned HTTP {status}")),
        }
        let body: Value = serde_json::from_str(&body)?;
        if body.get("code").and_then(Value::as_i64) == Some(0) {
            return Ok(body.get("result").cloned().unwrap_or(Value::Null));
        }
        let message = body.get("message").and_then(Value::as_str).unwrap_or_default();
        if message.to_lowercase().contains("auth") {
            return Err(Error::LoginRequired(self.relogin.clone()));
        }
        fail(format!("Xiaomi cloud error {}: {message}", body.get("code").unwrap_or(&Value::Null)))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub name: String,
    pub model: String,
    pub did: String,
    /// `None` when the cloud does not say (common for Bluetooth devices).
    pub online: Option<bool>,
}

impl Device {
    pub fn online_label(&self) -> &'static str {
        match self.online {
            Some(true) => "online",
            Some(false) => "offline",
            None => "online state unknown",
        }
    }
}

fn as_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

pub fn device_list(api: &dyn Api) -> Result<Vec<Device>> {
    let result = api.call("/home/device_list", &json!({"getVirtualModel": false, "getHuamiDevices": 0}))?;
    // Each entry also carries the device's local key, IP and Wi-Fi details. Keep only
    // what the CLI shows so the rest can never end up in an agent's context.
    let list = result.get("list").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(list
        .iter()
        .map(|d| Device {
            name: as_text(d.get("name")),
            model: as_text(d.get("model")),
            did: as_text(d.get("did")),
            online: d.get("isOnline").and_then(Value::as_bool),
        })
        .collect())
}

/// Reads properties by (siid, piid). A property the device did not return comes back as `None`.
pub fn get_props(api: &dyn Api, did: &str, iids: &[(u32, u32)]) -> Result<Vec<Option<Value>>> {
    let mut values = Vec::with_capacity(iids.len());
    for batch in iids.chunks(READ_BATCH) {
        let params: Vec<Value> = batch.iter().map(|(s, p)| json!({"did": did, "siid": s, "piid": p})).collect();
        let result = api.call("/miotspec/prop/get", &json!({ "params": params }))?;
        let items = result.as_array().cloned().unwrap_or_default();
        for (siid, piid) in batch {
            let item = items.iter().find(|it| {
                it.get("siid").and_then(Value::as_u64) == Some(u64::from(*siid))
                    && it.get("piid").and_then(Value::as_u64) == Some(u64::from(*piid))
            });
            values.push(
                item.filter(|it| it.get("code").and_then(Value::as_i64) == Some(0))
                    .and_then(|it| it.get("value").cloned()),
            );
        }
    }
    Ok(values)
}

/// Writes one property and returns the MiOT result code: 0 done, 1 accepted (the device
/// applies it asynchronously), negative values are errors.
pub fn set_prop(api: &dyn Api, did: &str, siid: u32, piid: u32, value: &Value) -> Result<i64> {
    let params = json!([{"did": did, "siid": siid, "piid": piid, "value": value}]);
    let result = api.call("/miotspec/prop/set", &json!({ "params": params }))?;
    Ok(result.get(0).and_then(|it| it.get("code")).and_then(Value::as_i64).unwrap_or(-1))
}

pub struct ActionResult {
    pub code: i64,
    pub out: Vec<Value>,
}

pub fn action(api: &dyn Api, did: &str, siid: u32, aiid: u32, args: &[Value]) -> Result<ActionResult> {
    let params = json!({"did": did, "siid": siid, "aiid": aiid, "in": args});
    let result = api.call("/miotspec/action", &json!({ "params": params }))?;
    Ok(ActionResult {
        code: result.get("code").and_then(Value::as_i64).unwrap_or(-1),
        out: result.get("out").and_then(Value::as_array).cloned().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_matches_reference_implementation() {
        // Expected values computed with MiService's MiIOService.sign_data for the same inputs.
        let ssecurity = B64.decode("dGVzdC1zc2VjdXJpdHktMTIzNA==").unwrap();
        let nonce = B64.decode("AQIDBAUGBwgBtE3A").unwrap();
        let (nonce_b64, signature) = sign("/miotspec/prop/get", r#"{"params":[]}"#, &ssecurity, &nonce);
        assert_eq!(nonce_b64, "AQIDBAUGBwgBtE3A");
        assert_eq!(signature, "UogANyFHVZvItRQ/j6FXJfVrkY2rAMO/elBPELohCmA=");
    }

    #[test]
    fn nonce_ends_with_current_minute() {
        let nonce = new_nonce();
        let minutes = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() / 60;
        let got = u32::from_be_bytes(nonce[8..].try_into().unwrap());
        assert!((minutes as u32).abs_diff(got) <= 1);
    }

    /// A local HTTP server that answers each connection with the next reply, or closes it
    /// without answering (`None`), the way the cloud drops an idle keep-alive connection.
    fn fake_server(replies: Vec<Option<&'static str>>) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/app", listener.local_addr().unwrap());
        let connections = std::sync::Arc::new(AtomicUsize::new(0));
        let seen = connections.clone();
        std::thread::spawn(move || {
            for reply in replies {
                let (stream, _) = listener.accept().unwrap();
                seen.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(stream);
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                reader.read_exact(&mut vec![0; length]).unwrap();
                if let Some(body) = reply {
                    let mut stream = reader.into_inner();
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .unwrap();
                }
            }
        });
        (url, connections)
    }

    fn cloud_at(server: String) -> Cloud {
        let token = Token {
            user_id: "1".into(),
            device_id: "TESTDEVICE".into(),
            ssecurity: "c2VjcmV0".into(),
            service_token: "t".into(),
        };
        Cloud { agent: http_agent(), server, token, ssecurity: b"secret".to_vec(), relogin: "login".into() }
    }

    #[test]
    fn reads_retry_once_when_the_connection_drops() {
        use std::sync::atomic::Ordering;
        let (url, connections) = fake_server(vec![None, Some(r#"{"code":0,"result":{"list":[]}}"#)]);
        assert!(device_list(&cloud_at(url)).unwrap().is_empty());
        assert_eq!(connections.load(Ordering::SeqCst), 2);

        let (url, connections) = fake_server(vec![None, None]);
        let err = device_list(&cloud_at(url)).unwrap_err().to_string();
        assert!(err.contains("cannot reach the Xiaomi cloud"), "{err}");
        assert_eq!(connections.load(Ordering::SeqCst), 2, "only one retry");
    }

    #[test]
    fn writes_are_never_retried() {
        use std::sync::atomic::Ordering;
        let (url, connections) = fake_server(vec![None]);
        let err = set_prop(&cloud_at(url), "1", 2, 1, &json!(true)).unwrap_err();
        assert!(matches!(err, Error::Interrupted(_)), "{err}");
        assert!(err.to_string().contains("may or may not have reached the device"), "{err}");
        assert_eq!(connections.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn region_servers() {
        assert_eq!(server("cn"), "https://api.io.mi.com/app");
        assert_eq!(server("de"), "https://de.api.io.mi.com/app");
    }
}
