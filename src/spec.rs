//! MiOT specs. Every model publishes on miot-spec.org which services, properties
//! (`siid-piid`) and actions (`siid-aiid`) it has, with formats, ranges and allowed values.
//! The CLI fetches the spec of a model once and caches it, so an agent can look up what any
//! device supports before touching it, instead of relying on a hard-coded model list.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{fail, Error, Result};

const SPEC_HOST: &str = "https://miot-spec.org/miot-spec-v2";
/// How long the model → spec index stays fresh. Specs themselves are versioned and never expire.
const INDEX_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
const MAX_DOWNLOAD: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct Spec {
    #[serde(rename = "type")]
    pub urn: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub services: Vec<Service>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Service {
    pub iid: u32,
    #[serde(rename = "type")]
    pub urn: String,
    #[serde(default)]
    pub properties: Vec<Property>,
    #[serde(default)]
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Property {
    pub iid: u32,
    #[serde(rename = "type")]
    pub urn: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub access: Vec<String>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(rename = "value-range", default)]
    pub value_range: Option<Vec<f64>>,
    #[serde(rename = "value-list", default)]
    pub value_list: Vec<ValueItem>,
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ValueItem {
    pub value: Value,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Action {
    pub iid: u32,
    #[serde(rename = "type")]
    pub urn: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "in", default)]
    pub inputs: Vec<u32>,
    #[serde(default)]
    pub out: Vec<u32>,
    #[serde(default)]
    pub comment: Option<String>,
}

/// `urn:miot-spec-v2:property:on:00000006:...`: the fourth field is the name.
pub fn urn_name(urn: &str) -> &str {
    urn.split(':').nth(3).unwrap_or_default()
}

/// Services that only describe the hardware (serial number, firmware). `status` skips them
/// unless asked, since serial numbers identify the device.
pub fn is_info_service(service: &Service) -> bool {
    urn_name(&service.urn) == "device-information"
}

pub trait Item {
    fn iid(&self) -> u32;
    fn urn(&self) -> &str;
    fn description(&self) -> &str;
    fn comment(&self) -> Option<&str>;
}

macro_rules! impl_item {
    ($t:ty) => {
        impl Item for $t {
            fn iid(&self) -> u32 {
                self.iid
            }
            fn urn(&self) -> &str {
                &self.urn
            }
            fn description(&self) -> &str {
                &self.description
            }
            fn comment(&self) -> Option<&str> {
                self.comment.as_deref()
            }
        }
    };
}
impl_item!(Property);
impl_item!(Action);

/// `2-1`
pub fn item_id(service: &Service, item: &impl Item) -> String {
    format!("{}-{}", service.iid, item.iid())
}

/// `switch.on`
pub fn item_name(service: &Service, item: &impl Item) -> String {
    format!("{}.{}", urn_name(&service.urn), urn_name(item.urn()))
}

/// `Switch Status (开关)`: the English description plus the Chinese comment when there is one.
pub fn item_label(item: &impl Item) -> String {
    let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let description = one_line(item.description());
    match item.comment().map(one_line).filter(|c| !c.is_empty() && *c != description) {
        Some(c) if description.is_empty() => c,
        Some(c) => format!("{description} ({c})"),
        None => description,
    }
}

impl Spec {
    /// Parses a spec and orders services and their items by iid, so listings read 1-1, 1-2, 2-1...
    pub fn parse(text: &str) -> Result<Spec> {
        let mut spec: Spec = serde_json::from_str(text)?;
        spec.services.sort_by_key(|s| s.iid);
        for service in &mut spec.services {
            service.properties.sort_by_key(|p| p.iid);
            service.actions.sort_by_key(|a| a.iid);
        }
        Ok(spec)
    }

    /// Device kind from the spec type, e.g. `air-purifier`, `outlet`, `lock`.
    pub fn kind(&self) -> &str {
        urn_name(&self.urn)
    }

    pub fn properties(&self) -> impl Iterator<Item = (&Service, &Property)> {
        self.services.iter().flat_map(|s| s.properties.iter().map(move |p| (s, p)))
    }

    pub fn actions(&self) -> impl Iterator<Item = (&Service, &Action)> {
        self.services.iter().flat_map(|s| s.actions.iter().map(move |a| (s, a)))
    }

    /// The properties an action takes or returns live in the action's own service.
    pub fn action_params<'a>(&'a self, service: &'a Service, piids: &[u32]) -> Result<Vec<&'a Property>> {
        piids
            .iter()
            .map(|piid| {
                service.properties.iter().find(|p| p.iid == *piid).ok_or_else(|| {
                    Error::Other(format!("the spec is inconsistent: property {}-{piid} is missing", service.iid))
                })
            })
            .collect()
    }

    pub fn find_property(&self, key: &str) -> Result<(&Service, &Property)> {
        find(self.properties(), key, "property")
    }

    pub fn find_action(&self, key: &str) -> Result<(&Service, &Action)> {
        find(self.actions(), key, "action")
    }
}

/// Accepts `siid-iid` (`2-1`), `service.name` (`switch.on`) or a bare name (`on`), any case.
fn find<'a, T: Item>(
    items: impl Iterator<Item = (&'a Service, &'a T)>,
    key: &str,
    word: &str,
) -> Result<(&'a Service, &'a T)> {
    let key = key.trim().to_lowercase();
    let hits: Vec<_> = items
        .filter(|(s, it)| key == item_id(s, *it) || key == item_name(s, *it) || key == urn_name(it.urn()))
        .collect();
    match hits.as_slice() {
        [hit] => Ok(*hit),
        [] => fail(format!("this device has no {word} {key:?}; run `mijia spec <device>` to see what it has")),
        _ => fail(format!(
            "{key:?} matches several {word}s: {}; use the siid-iid form",
            hits.iter()
                .map(|(s, it)| format!("{} {}", item_id(s, *it), item_name(s, *it)))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

impl Property {
    pub fn readable(&self) -> bool {
        self.access.iter().any(|a| a == "read")
    }

    pub fn writable(&self) -> bool {
        self.access.iter().any(|a| a == "write")
    }

    /// `r`, `w`, `rw`, or `-` when the spec grants neither (some properties only appear in events).
    pub fn access_label(&self) -> &'static str {
        match (self.readable(), self.writable()) {
            (true, true) => "rw",
            (true, false) => "r",
            (false, true) => "w",
            (false, false) => "-",
        }
    }

    fn is_integer(&self) -> bool {
        self.format.starts_with("int") || self.format.starts_with("uint")
    }

    fn unit_suffix(&self) -> String {
        match self.unit.as_deref() {
            None | Some("none") | Some("") => String::new(),
            Some("percentage") => "%".into(),
            Some("celsius") => "°C".into(),
            Some(unit) => format!(" {unit}"),
        }
    }

    /// What a valid value looks like, for usage hints and error messages.
    pub fn value_hint(&self) -> String {
        if self.format == "bool" {
            return "true/false".into();
        }
        if !self.value_list.is_empty() {
            return self
                .value_list
                .iter()
                .map(|v| format!("{}={}", value_text(&v.value), v.description))
                .collect::<Vec<_>>()
                .join(", ");
        }
        if let Some([min, max, rest @ ..]) = self.value_range.as_deref() {
            let step = rest.first().filter(|s| **s != 1.0).map(|s| format!(" step {s}")).unwrap_or_default();
            return format!("{min}..{max}{step}{}", self.unit_suffix());
        }
        self.format.clone()
    }

    /// A value as people read it: `1 (Level1)`, `23.5°C`, `true`.
    pub fn show(&self, value: &Value) -> String {
        if value.is_null() {
            return "(no value)".into();
        }
        if let Some(item) = self.value_list.iter().find(|v| same_value(&v.value, value)) {
            return match item.comment.as_deref().filter(|c| !c.is_empty() && *c != item.description) {
                Some(c) => format!("{} ({}, {c})", value_text(value), item.description),
                None => format!("{} ({})", value_text(value), item.description),
            };
        }
        if value.is_number() {
            return format!("{}{}", value_text(value), self.unit_suffix());
        }
        value_text(value)
    }

    /// Turns command-line text into a value of this property's type, checking ranges,
    /// steps and allowed values before anything is sent to a device.
    pub fn parse(&self, text: &str) -> Result<Value> {
        let trimmed = text.trim();
        if self.format == "bool" {
            return match trimmed.to_lowercase().as_str() {
                "true" | "on" | "1" | "yes" | "开" => Ok(Value::Bool(true)),
                "false" | "off" | "0" | "no" | "关" => Ok(Value::Bool(false)),
                _ => fail(format!("{text:?} is not a boolean; use true or false")),
            };
        }
        if !self.value_list.is_empty() {
            return self
                .value_list
                .iter()
                .find(|v| {
                    value_text(&v.value) == trimmed
                        || v.description.eq_ignore_ascii_case(trimmed)
                        || v.comment.as_deref() == Some(trimmed)
                })
                .map(|v| v.value.clone())
                .ok_or_else(|| {
                    Error::Other(format!("{text:?} is not an allowed value; use one of: {}", self.value_hint()))
                });
        }
        if !self.is_integer() && self.format != "float" {
            return Ok(Value::from(text));
        }
        let number: f64 = trimmed.parse().map_err(|_| Error::Other(format!("{text:?} is not a number")))?;
        if !number.is_finite() || (self.is_integer() && number.fract() != 0.0) {
            return fail(format!("{text:?} is not a whole number"));
        }
        if let Some([min, max, rest @ ..]) = self.value_range.as_deref() {
            if number < *min || number > *max {
                return fail(format!("{text} is out of range {}", self.value_hint()));
            }
            if let Some(step) = rest.first().filter(|s| **s > 0.0) {
                let steps = (number - min) / step;
                if (steps - steps.round()).abs() > 1e-6 {
                    return fail(format!("{text} is not a valid step; allowed: {}", self.value_hint()));
                }
            }
        }
        Ok(if self.is_integer() { json!(number as i64) } else { json!(number) })
    }

    pub fn describe(&self) -> Value {
        json!({
            "format": self.format,
            "access": self.access,
            "unit": self.unit,
            "range": self.value_range,
            "values": self.value_list.iter().map(|v| json!({
                "value": v.value, "description": v.description, "comment": v.comment,
            })).collect::<Vec<_>>(),
            "hint": self.value_hint(),
        })
    }
}

pub fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => match n.as_f64() {
            // Show 26.0 as 26 but keep 26.5.
            Some(f) if !n.is_i64() && !n.is_u64() && f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

/// Numbers compare by value, so a read-back `26.0` matches a written `26`.
pub fn same_value(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-9,
        _ => a == b,
    }
}

/// Device kinds whose writes can hurt people, property or privacy: locks and access, gas and
/// valves, anything that heats, cameras, and network gear. `set` and `action` on these refuse
/// to run without `--confirmed`, which an agent may only add after the user said yes.
const RISKY_KINDS: &[(&str, &str)] = &[
    ("lock", "lock / access"),
    ("door", "lock / access"),
    ("gate", "lock / access"),
    ("garage", "lock / access"),
    ("window", "lock / access"),
    ("safe", "lock / access"),
    ("gas", "gas / valve"),
    ("valve", "gas / valve"),
    ("heater", "heating"),
    ("oven", "heating"),
    ("cooker", "heating"),
    ("stove", "heating"),
    ("hob", "heating"),
    ("kettle", "heating"),
    ("pot", "heating"),
    ("blanket", "heating"),
    ("boiler", "heating"),
    ("fryer", "heating"),
    ("toaster", "heating"),
    ("grill", "heating"),
    ("steamer", "heating"),
    ("fireplace", "heating"),
    ("camera", "camera / privacy"),
    ("router", "network"),
    ("gateway", "network"),
];

/// The risk category of a device, judged by its spec type (not its user-editable name).
pub fn risk(spec: &Spec) -> Option<&'static str> {
    spec.kind().split('-').find_map(|word| RISKY_KINDS.iter().find(|(w, _)| *w == word).map(|(_, category)| *category))
}

/// Where specs come from. A trait so commands can be tested with fixture specs.
pub trait SpecSource {
    fn for_model(&self, model: &str) -> Result<Spec>;
}

/// Downloads specs from miot-spec.org (public data, no login needed) and caches them on disk.
pub struct SpecStore {
    dir: PathBuf,
    agent: ureq::Agent,
}

impl SpecStore {
    pub fn new(dir: &Path) -> SpecStore {
        SpecStore { dir: dir.to_path_buf(), agent: crate::cloud::http_agent() }
    }

    fn download(&self, url: &str) -> Result<String> {
        let mut response = self
            .agent
            .get(url)
            .header("User-Agent", concat!("mijia-cli/", env!("CARGO_PKG_VERSION")))
            .call()
            .map_err(|e| Error::Other(format!("cannot reach miot-spec.org: {e}")))?;
        let status = response.status().as_u16();
        if status != 200 {
            return fail(format!("miot-spec.org returned HTTP {status} for {url}"));
        }
        Ok(response.body_mut().with_config().limit(MAX_DOWNLOAD).read_to_string()?)
    }

    /// Model → spec type (URN), keeping the highest version of each model.
    /// `status` is `released`, or `all` to include preview and debug specs.
    fn index(&self, status: &str, refresh: bool) -> Result<(HashMap<String, String>, bool)> {
        let path = self.dir.join(format!("models-{status}.json"));
        let age =
            fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok());
        if !refresh && age.is_some_and(|a| a < INDEX_MAX_AGE) {
            if let Ok(map) = fs::read_to_string(&path).map_err(Error::from).and_then(|t| Ok(serde_json::from_str(&t)?))
            {
                return Ok((map, false));
            }
        }
        let text = match self.download(&format!("{SPEC_HOST}/instances?status={status}")) {
            Ok(text) => text,
            Err(e) => {
                // Offline: a stale index is better than none.
                if let Ok(map) =
                    fs::read_to_string(&path).map_err(Error::from).and_then(|t| Ok(serde_json::from_str(&t)?))
                {
                    return Ok((map, false));
                }
                return Err(e);
            }
        };
        let instances: Value = serde_json::from_str(&text)?;
        let mut best: HashMap<String, (u64, String)> = HashMap::new();
        for it in instances.get("instances").and_then(Value::as_array).into_iter().flatten() {
            let (Some(model), Some(urn)) =
                (it.get("model").and_then(Value::as_str), it.get("type").and_then(Value::as_str))
            else {
                continue;
            };
            let version = it.get("version").and_then(Value::as_u64).unwrap_or(0);
            if best.get(model).is_none_or(|(v, _)| version > *v) {
                best.insert(model.to_string(), (version, urn.to_string()));
            }
        }
        let map: HashMap<String, String> = best.into_iter().map(|(m, (_, urn))| (m, urn)).collect();
        write_atomic(&path, &serde_json::to_string(&map)?)?;
        Ok((map, true))
    }

    fn urn_for_model(&self, model: &str) -> Result<String> {
        for status in ["released", "all"] {
            let (map, fresh) = self.index(status, false)?;
            if let Some(urn) = map.get(model) {
                return Ok(urn.clone());
            }
            if !fresh {
                // The cached index may predate this model.
                if let Some(urn) = self.index(status, true)?.0.get(model) {
                    return Ok(urn.clone());
                }
            }
        }
        fail(format!(
            "miot-spec.org has no MiOT spec for model {model:?}. It may be a legacy miIO device, \
             a non-Xiaomi integration or a misspelled model; this tool can only control MiOT devices."
        ))
    }

    fn for_urn(&self, urn: &str) -> Result<Spec> {
        let path = self.dir.join("specs").join(format!("{}.json", urn.replace([':', '/', '\\'], "_")));
        if let Ok(text) = fs::read_to_string(&path) {
            if let Ok(spec) = Spec::parse(&text) {
                return Ok(spec);
            }
        }
        let text = self.download(&format!("{SPEC_HOST}/instance?type={urn}"))?;
        let spec = Spec::parse(&text)?;
        write_atomic(&path, &text)?;
        Ok(spec)
    }
}

impl SpecSource for SpecStore {
    fn for_model(&self, model: &str) -> Result<Spec> {
        let urn = self.urn_for_model(model)?;
        self.for_urn(&urn)
    }
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn fixture() -> Spec {
        Spec::parse(include_str!("../tests/fixtures/heater-spec.json")).unwrap()
    }

    #[test]
    fn names_and_lookup() {
        let spec = fixture();
        assert_eq!(spec.kind(), "heater");
        let (s, p) = spec.find_property("heater.on").unwrap();
        assert_eq!(item_id(s, p), "2-1");
        assert_eq!(spec.find_property("2-1").unwrap().1.iid, 1);
        assert_eq!(spec.find_property("TARGET-TEMPERATURE").unwrap().1.iid, 2);
        // `on` exists in two services, so the bare name is ambiguous.
        let err = spec.find_property("on").unwrap_err().to_string();
        assert!(err.contains("2-1") && err.contains("3-1"), "{err}");
        assert!(spec.find_property("nope").is_err());
        let (s, a) = spec.find_action("set-timer").unwrap();
        assert_eq!(item_id(s, a), "2-1");
        assert_eq!(spec.action_params(s, &a.inputs).unwrap()[0].iid, 4);
    }

    #[test]
    fn parses_values_by_type() {
        let spec = fixture();
        let on = spec.find_property("2-1").unwrap().1;
        assert_eq!(on.parse("on").unwrap(), json!(true));
        assert_eq!(on.parse("关").unwrap(), json!(false));
        assert!(on.parse("maybe").is_err());

        let temp = spec.find_property("target-temperature").unwrap().1;
        assert_eq!(temp.parse("22.5").unwrap(), json!(22.5));
        assert!(temp.parse("22.3").is_err(), "off-step value must be rejected");
        assert!(temp.parse("40").is_err(), "out-of-range value must be rejected");

        let mode = spec.find_property("mode").unwrap().1;
        assert_eq!(mode.parse("2").unwrap(), json!(2));
        assert_eq!(mode.parse("eco").unwrap(), json!(2));
        assert_eq!(mode.parse("节能").unwrap(), json!(2));
        assert!(mode.parse("turbo").is_err());

        let minutes = spec.find_property("countdown-time").unwrap().1;
        assert_eq!(minutes.parse("90").unwrap(), json!(90));
        assert!(minutes.parse("1.5").is_err());
    }

    #[test]
    fn access_labels() {
        let spec = fixture();
        assert_eq!(spec.find_property("heater.on").unwrap().1.access_label(), "rw");
        assert_eq!(spec.find_property("heater.temperature").unwrap().1.access_label(), "r");
        assert_eq!(spec.find_property("heater.alarm").unwrap().1.access_label(), "w");
        assert_eq!(spec.find_property("heater.status-code").unwrap().1.access_label(), "-");
    }

    #[test]
    fn shows_values() {
        let spec = fixture();
        let temp = spec.find_property("target-temperature").unwrap().1;
        assert_eq!(temp.show(&json!(22.5)), "22.5°C");
        assert_eq!(temp.value_hint(), "16..32 step 0.5°C");
        let mode = spec.find_property("mode").unwrap().1;
        assert_eq!(mode.show(&json!(2)), "2 (Eco, 节能)");
        assert_eq!(mode.show(&Value::Null), "(no value)");
        assert_eq!(value_text(&json!(26.0)), "26");
        assert!(same_value(&json!(26), &json!(26.0)));
    }

    #[test]
    fn risk_comes_from_the_spec_type() {
        let mut spec = fixture();
        assert_eq!(risk(&spec), Some("heating"));
        spec.urn = "urn:miot-spec-v2:device:outlet:0000A002:demo-plug:1".into();
        assert_eq!(risk(&spec), None);
        spec.urn = "urn:miot-spec-v2:device:lock:0000A038:demo-lock:1".into();
        assert_eq!(risk(&spec), Some("lock / access"));
        spec.urn = "urn:miot-spec-v2:device:water-heater:0000A02A:demo:1".into();
        assert_eq!(risk(&spec), Some("heating"));
        spec.urn = "urn:miot-spec-v2:device:alarm-clock:0000A0FF:demo:1".into();
        assert_eq!(risk(&spec), None, "\"clock\" must not match \"lock\"");
    }
}
