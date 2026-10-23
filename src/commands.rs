//! The device commands. Each returns text for people and JSON for programs.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use crate::cloud::{self, Api, Device};
use crate::error::{fail, Error, Result};
use crate::spec::{self, item_id, item_label, item_name, Action, Property, Service, Spec, SpecSource};

#[derive(Debug)]
pub struct Output {
    pub text: String,
    pub json: Value,
}

/// After a write the cloud only shows the new value once the device reports it, which can
/// take a few seconds, so `set` polls a few times before reporting what it read back.
pub struct Readback {
    pub wait: Duration,
    pub tries: u32,
}

impl Default for Readback {
    fn default() -> Self {
        Readback { wait: Duration::from_millis(1500), tries: 4 }
    }
}

/// Matches an exact did or name first, then a unique part of a name (any case).
pub fn find_device<'a>(devices: &'a [Device], key: &str) -> Result<&'a Device> {
    let key = key.trim();
    let lower = key.to_lowercase();
    let mut hits: Vec<&Device> = devices.iter().filter(|d| d.did == key || d.name.to_lowercase() == lower).collect();
    if hits.is_empty() {
        hits = devices.iter().filter(|d| d.name.to_lowercase().contains(&lower)).collect();
    }
    match hits.as_slice() {
        [device] => Ok(device),
        [] => fail(format!("no device matches {key:?}; run `mijia list` to see device names")),
        many => fail(format!(
            "{key:?} matches several devices: {}; use the full name or the did",
            many.iter().map(|d| format!("{} (did {})", d.name, d.did)).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Terminal columns a string takes: CJK and full-width characters take two.
fn width(s: &str) -> usize {
    s.chars()
        .map(|c| match c as u32 {
            0x1100..=0x115F
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6 => 2,
            _ => 1,
        })
        .sum()
}

/// Long values (lists of connected clients, JSON blobs) would wreck the table; the full value is in `--json`.
fn clip(text: String) -> String {
    const MAX: usize = 60;
    if text.chars().count() <= MAX {
        return text;
    }
    let head: String = text.chars().take(MAX).collect();
    format!("{head}... ({} chars; see --json)", text.chars().count())
}

/// Lines of columns, padded so the columns line up; the last column is not padded.
fn table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> =
        (0..columns).map(|c| rows.iter().filter_map(|r| r.get(c)).map(|s| width(s)).max().unwrap_or(0)).collect();
    rows.iter()
        .map(|row| {
            let mut line = String::from(" ");
            for (c, cell) in row.iter().enumerate() {
                line.push(' ');
                line.push_str(cell);
                if c + 1 < row.len() {
                    line.push_str(&" ".repeat(widths[c] - width(cell) + 1));
                }
            }
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn risk_note(spec: &Spec) -> String {
    spec::risk(spec)
        .map(|category| format!(" | HIGH RISK ({category}): writes need the user's confirmation"))
        .unwrap_or_default()
}

pub fn list(api: &dyn Api) -> Result<Output> {
    let devices = cloud::device_list(api)?;
    let text = if devices.is_empty() {
        "No devices on this account in this region. If the Mi Home app shows devices, \
         the region is probably wrong: try --region (cn, de, i2, ru, sg, us, tw, in)."
            .to_string()
    } else {
        devices
            .iter()
            .map(|d| format!("{} | {} | did {} | {}", d.name, d.model, d.did, d.online_label()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(Output { text, json: json!(devices) })
}

fn args_hint(spec: &Spec, service: &Service, piids: &[u32]) -> Result<String> {
    let params = spec.action_params(service, piids)?;
    if params.is_empty() {
        return Ok("no arguments".into());
    }
    Ok(params.iter().map(|p| format!("{}: {}", spec::urn_name(&p.urn), p.value_hint())).collect::<Vec<_>>().join(", "))
}

fn params_json(spec: &Spec, service: &Service, piids: &[u32]) -> Result<Vec<Value>> {
    Ok(spec
        .action_params(service, piids)?
        .iter()
        .map(|p| json!({"id": item_id(service, *p), "name": item_name(service, *p), "hint": p.value_hint()}))
        .collect())
}

fn action_json(spec: &Spec, service: &Service, action: &Action) -> Result<Value> {
    Ok(json!({
        "id": item_id(service, action), "name": item_name(service, action),
        "description": action.description, "comment": action.comment, "label": item_label(action),
        "in": params_json(spec, service, &action.inputs)?, "out": params_json(spec, service, &action.out)?,
    }))
}

fn spec_output(spec: &Spec, model: &str, device: Option<&Device>) -> Result<Output> {
    let header = match device {
        Some(d) => format!("{} | {model} | {}", d.name, spec.description),
        None => format!("{model} | {}", spec.description),
    };
    let mut text =
        format!("{header} | kind {}{}\nproperties (r = read, w = write, - = neither):\n", spec.kind(), risk_note(spec));
    let rows: Vec<Vec<String>> = spec
        .properties()
        .map(|(s, p)| vec![item_id(s, p), item_name(s, p), p.access_label().into(), item_label(p), p.value_hint()])
        .collect();
    text += &table(&rows);
    let mut actions = Vec::new();
    let mut action_rows = Vec::new();
    for (s, a) in spec.actions() {
        action_rows.push(vec![
            item_id(s, a),
            item_name(s, a),
            item_label(a),
            format!("({})", args_hint(spec, s, &a.inputs)?),
        ]);
        actions.push(action_json(spec, s, a)?);
    }
    if !action_rows.is_empty() {
        text += "\nactions (run with: mijia action <device> <id> [args...]):\n";
        text += &table(&action_rows);
    }
    let properties: Vec<Value> = spec
        .properties()
        .map(|(s, p)| {
            let mut v = p.describe();
            v["id"] = json!(item_id(s, p));
            v["name"] = json!(item_name(s, p));
            v["description"] = json!(p.description);
            v["comment"] = json!(p.comment);
            v["label"] = json!(item_label(p));
            v
        })
        .collect();
    let json = json!({
        "device": device, "model": model, "type": spec.urn, "kind": spec.kind(),
        "description": spec.description, "risk": spec::risk(spec),
        "properties": properties, "actions": actions,
    });
    Ok(Output { text, json })
}

pub fn spec_of_model(specs: &dyn SpecSource, model: &str) -> Result<Output> {
    spec_output(&specs.for_model(model)?, model, None)
}

pub fn spec_of_device(api: &dyn Api, specs: &dyn SpecSource, key: &str) -> Result<Output> {
    let devices = cloud::device_list(api)?;
    let device = find_device(&devices, key).map_err(|e| {
        Error::Other(format!("{e}. To look up a model instead of a device, use `mijia spec --model <model>`"))
    })?;
    spec_output(&specs.for_model(&device.model)?, &device.model, Some(device))
}

pub fn status(api: &dyn Api, specs: &dyn SpecSource, key: &str, keys: &[String]) -> Result<Output> {
    let devices = cloud::device_list(api)?;
    let device = find_device(&devices, key)?;
    let spec = specs.for_model(&device.model)?;
    let selected: Vec<(&Service, &Property)> = if keys.is_empty() {
        spec.properties().filter(|(s, p)| p.readable() && !spec::is_info_service(s)).collect()
    } else {
        let mut chosen = Vec::new();
        for k in keys {
            let (s, p) = spec.find_property(k)?;
            if !p.readable() {
                let why =
                    if p.writable() { "is write-only" } else { "has no read access in its spec (access is empty)" };
                return fail(format!("{} {} {why} and cannot be read", item_id(s, p), item_name(s, p)));
            }
            chosen.push((s, p));
        }
        chosen
    };
    // The firmware version rides along in the same request: it is how firmware updates get noticed.
    let firmware_prop = spec
        .properties()
        .find(|(s, p)| spec::is_info_service(s) && spec::urn_name(&p.urn) == "firmware-revision" && p.readable());
    let mut iids: Vec<(u32, u32)> = selected.iter().map(|(s, p)| (s.iid, p.iid)).collect();
    if let Some((s, p)) = firmware_prop {
        iids.push((s.iid, p.iid));
    }
    let mut values = cloud::get_props(api, &device.did, &iids)?;
    let firmware = if firmware_prop.is_some() { values.pop().flatten() } else { None };
    let firmware_note = match &firmware {
        Some(v) if !v.is_null() => format!(" | firmware {}", spec::value_text(v)),
        _ => String::new(),
    };

    let mut text =
        format!("{} | {} | {}{firmware_note}{}\n", device.name, device.model, device.online_label(), risk_note(&spec));
    let mut rows = Vec::new();
    let mut properties = Vec::new();
    for (&(s, p), value) in selected.iter().zip(&values) {
        let value = value.clone().unwrap_or(Value::Null);
        let access = if p.writable() { format!("[rw {}]", p.value_hint()) } else { "[r]".into() };
        rows.push(vec![item_id(s, p), item_name(s, p), format!("= {}", clip(p.show(&value))), item_label(p), access]);
        properties.push(json!({
            "id": item_id(s, p), "name": item_name(s, p), "description": p.description,
            "comment": p.comment, "label": item_label(p),
            "value": value, "display": p.show(&value), "writable": p.writable(), "hint": p.value_hint(),
        }));
    }
    text += &if rows.is_empty() { "  (no readable properties)".to_string() } else { table(&rows) };
    let mut json = json!({
        "device": device, "kind": spec.kind(), "risk": spec::risk(&spec), "firmware": firmware,
        "properties": properties,
    });
    // A full status (no properties named) also lists what can be written or run, in text and JSON alike.
    if keys.is_empty() {
        let write_only: Vec<(&Service, &Property)> =
            spec.properties().filter(|(_, p)| p.writable() && !p.readable()).collect();
        if !write_only.is_empty() {
            let rows: Vec<Vec<String>> = write_only
                .iter()
                .map(|&(s, p)| vec![item_id(s, p), item_name(s, p), item_label(p), format!("[w {}]", p.value_hint())])
                .collect();
            text += "\nwrite-only:\n";
            text += &table(&rows);
        }
        json["write_only"] = write_only
            .iter()
            .map(|&(s, p)| {
                json!({
                    "id": item_id(s, p), "name": item_name(s, p), "description": p.description,
                    "comment": p.comment, "label": item_label(p), "hint": p.value_hint(),
                })
            })
            .collect();
        let mut action_rows = Vec::new();
        let mut actions = Vec::new();
        for (s, a) in spec.actions() {
            action_rows.push(vec![item_id(s, a), item_name(s, a), format!("({})", args_hint(&spec, s, &a.inputs)?)]);
            actions.push(action_json(&spec, s, a)?);
        }
        if !action_rows.is_empty() {
            text += "\nactions:\n";
            text += &table(&action_rows);
        }
        json["actions"] = Value::Array(actions);
    }
    Ok(Output { text, json })
}

fn check_risk(spec: &Spec, device: &Device, confirmed: bool, what: &str) -> Result<()> {
    match spec::risk(spec) {
        Some(category) if !confirmed => Err(Error::NeedsConfirmation(format!(
            "{} is a {} device (high risk: {category}); nothing was sent. Ask the user to confirm this \
             exact operation ({what}). Only after they say yes, run the same command again with --confirmed.",
            device.name,
            spec.kind()
        ))),
        _ => Ok(()),
    }
}

fn require_online(device: &Device) -> Result<()> {
    if device.online == Some(false) {
        return fail(format!("{} is offline; nothing was sent", device.name));
    }
    Ok(())
}

/// MiOT result codes: 0 done, 1 accepted (the device applies it asynchronously).
fn result_label(device: &Device, what: &str, code: i64) -> Result<&'static str> {
    match code {
        0 => Ok("done"),
        1 => Ok("accepted"),
        _ => fail(format!("{}: {what} was rejected (MiOT code {code})", device.name)),
    }
}

pub fn set(
    api: &dyn Api,
    specs: &dyn SpecSource,
    key: &str,
    property: &str,
    text: &str,
    confirmed: bool,
    readback: &Readback,
) -> Result<Output> {
    let devices = cloud::device_list(api)?;
    let device = find_device(&devices, key)?;
    let spec = specs.for_model(&device.model)?;
    let (s, p) = spec.find_property(property)?;
    let (id, name) = (item_id(s, p), item_name(s, p));
    if !p.writable() {
        return fail(format!("{id} {name} is read-only"));
    }
    let value = p.parse(text)?;
    let what = format!("set {id} {name} to {} on {}", p.show(&value), device.name);
    check_risk(&spec, device, confirmed, &what)?;
    require_online(device)?;
    let mut resent = false;
    let code = match cloud::set_prop(api, &device.did, s.iid, p.iid, &value) {
        // Setting a property to a value is idempotent, so when the connection breaks before the
        // reply, read the property: if the value is not there yet, send it once more. If the
        // read fails too, the original error says best what to do.
        Err(Error::Interrupted(msg)) if p.readable() => {
            sleep(readback.wait);
            let now = cloud::get_props(api, &device.did, &[(s.iid, p.iid)]).map_err(|_| Error::Interrupted(msg))?;
            if spec::same_value(&now.into_iter().next().flatten().unwrap_or(Value::Null), &value) {
                None
            } else {
                resent = true;
                Some(cloud::set_prop(api, &device.did, s.iid, p.iid, &value)?)
            }
        }
        reply => Some(reply?),
    };
    let result = match code {
        Some(code) => result_label(device, &what, code)?,
        None => "already set",
    };

    let mut line = format!("{}: {id} {name} -> {} ({result})", device.name, p.show(&value));
    if resent {
        line += "; sent twice: the connection broke before the first reply";
    } else if code.is_none() {
        line += "; the connection broke before the reply, but a read shows the value in place";
    }
    let mut read_back = Value::Null;
    let mut applied = None;
    if p.readable() {
        for _ in 0..readback.tries {
            sleep(readback.wait);
            read_back = cloud::get_props(api, &device.did, &[(s.iid, p.iid)])?.pop().flatten().unwrap_or(Value::Null);
            if spec::same_value(&read_back, &value) {
                break;
            }
        }
        let same = spec::same_value(&read_back, &value);
        applied = Some(same);
        line += &format!("; read back {}", p.show(&read_back));
        if !same {
            line += ". Not updated yet: the device may not have applied it. \
                     Check again later with `mijia status` before trying anything else";
        }
    }
    let json = json!({
        "device": device, "property": id, "name": name, "value": value, "display": p.show(&value),
        "code": code, "result": result, "resent": resent, "read_back": read_back, "applied": applied,
    });
    Ok(Output { text: line, json })
}

pub fn action(
    api: &dyn Api,
    specs: &dyn SpecSource,
    key: &str,
    action_key: &str,
    texts: &[String],
    confirmed: bool,
) -> Result<Output> {
    let devices = cloud::device_list(api)?;
    let device = find_device(&devices, key)?;
    let spec = specs.for_model(&device.model)?;
    let (s, a): (&Service, &Action) = spec.find_action(action_key)?;
    let (id, name) = (item_id(s, a), item_name(s, a));
    let params = spec.action_params(s, &a.inputs)?;
    if texts.len() != params.len() {
        return fail(format!(
            "{id} {name} takes {} argument(s) ({}), got {}",
            params.len(),
            args_hint(&spec, s, &a.inputs)?,
            texts.len()
        ));
    }
    let args = params.iter().zip(texts).map(|(p, t)| p.parse(t)).collect::<Result<Vec<_>>>()?;
    let shown: Vec<String> = params.iter().zip(&args).map(|(p, v)| p.show(v)).collect();
    let what = format!("run {id} {name}({}) on {}", shown.join(", "), device.name);
    check_risk(&spec, device, confirmed, &what)?;
    require_online(device)?;
    let reply = cloud::action(api, &device.did, s.iid, a.iid, &args)?;
    let result = result_label(device, &what, reply.code)?;

    let mut line = format!("{}: {id} {name} ({result})", device.name);
    let mut out = Vec::new();
    if let Ok(out_params) = spec.action_params(s, &a.out) {
        for (p, v) in out_params.iter().zip(&reply.out) {
            out.push(json!({"id": item_id(s, *p), "name": item_name(s, *p), "value": v, "display": p.show(v)}));
        }
        if !out.is_empty() {
            let shown: Vec<String> = out_params
                .iter()
                .zip(&reply.out)
                .map(|(p, v)| format!("{}={}", spec::urn_name(&p.urn), p.show(v)))
                .collect();
            line += &format!("; returned {}", shown.join(", "));
        }
    }
    let json = json!({
        "device": device, "action": id, "name": name, "args": args,
        "code": reply.code, "result": result, "out": out,
    });
    Ok(Output { text: line, json })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    /// A fake cloud with one heater ("Study Heater") and one plug, recording every call.
    struct FakeCloud {
        calls: RefCell<Vec<(String, Value)>>,
        heater_online: bool,
        applies_writes: bool,
        /// The value of property 2-1, changed by writes the device applies.
        on: Cell<bool>,
        /// Upcoming writes whose connection breaks before the reply: `true` if the write
        /// reached the device first, `false` if it was lost.
        broken_writes: RefCell<VecDeque<bool>>,
    }

    impl FakeCloud {
        fn new() -> Self {
            FakeCloud {
                calls: RefCell::new(Vec::new()),
                heater_online: true,
                applies_writes: true,
                on: Cell::new(false),
                broken_writes: RefCell::new(VecDeque::new()),
            }
        }
        fn breaking(writes: &[bool]) -> Self {
            FakeCloud { broken_writes: RefCell::new(writes.iter().copied().collect()), ..FakeCloud::new() }
        }
        fn writes(&self) -> Vec<Value> {
            self.calls
                .borrow()
                .iter()
                .filter(|(uri, _)| uri != "/home/device_list" && uri != "/miotspec/prop/get")
                .map(|(_, data)| data.clone())
                .collect()
        }
    }

    impl Api for FakeCloud {
        fn call(&self, uri: &str, data: &Value) -> Result<Value> {
            self.calls.borrow_mut().push((uri.to_string(), data.clone()));
            Ok(match uri {
                "/home/device_list" => json!({"list": [
                    {"name": "Study Heater", "model": "demo.heater.h1", "did": "1001", "isOnline": self.heater_online, "token": "LOCAL-KEY", "localip": "10.0.0.2"},
                    {"name": "Desk Plug", "model": "demo.plug.p1", "did": "1002", "isOnline": true},
                ]}),
                "/miotspec/prop/get" => {
                    let params = data["params"].as_array().unwrap();
                    Value::Array(
                        params
                            .iter()
                            .map(|p| {
                                let value = match (p["siid"].as_u64(), p["piid"].as_u64()) {
                                    (Some(2), Some(1)) => json!(self.on.get()),
                                    (Some(2), Some(2)) => json!(21.0),
                                    (Some(2), Some(3)) => json!(2),
                                    (Some(1), Some(3)) => json!("1.0.9"),
                                    _ => json!(0),
                                };
                                json!({"did": p["did"], "siid": p["siid"], "piid": p["piid"], "code": 0, "value": value})
                            })
                            .collect(),
                    )
                }
                "/miotspec/prop/set" | "/miotspec/action" => {
                    let broken = self.broken_writes.borrow_mut().pop_front();
                    let p = &data["params"][0];
                    if self.applies_writes
                        && broken != Some(false)
                        && (p["siid"].as_u64(), p["piid"].as_u64()) == (Some(2), Some(1))
                    {
                        self.on.set(p["value"].as_bool().unwrap());
                    }
                    if broken.is_some() {
                        return Err(Error::Interrupted(
                            "the connection broke; it may or may not have reached the device".into(),
                        ));
                    }
                    if uri == "/miotspec/action" {
                        json!({"code": 0, "out": [30]})
                    } else {
                        json!([{"code": 0}])
                    }
                }
                other => panic!("unexpected call {other}"),
            })
        }
    }

    struct FixtureSpecs;

    impl SpecSource for FixtureSpecs {
        fn for_model(&self, model: &str) -> Result<Spec> {
            let mut spec = spec::tests::fixture();
            if model == "demo.plug.p1" {
                spec.urn = "urn:miot-spec-v2:device:outlet:0000A002:demo-plug:1".into();
            }
            Ok(spec)
        }
    }

    const FAST: Readback = Readback { wait: Duration::ZERO, tries: 2 };

    #[test]
    fn list_keeps_only_safe_fields() {
        let out = list(&FakeCloud::new()).unwrap();
        assert!(out.text.contains("Study Heater | demo.heater.h1 | did 1001 | online"));
        let dumped = out.json.to_string();
        assert!(!dumped.contains("LOCAL-KEY") && !dumped.contains("10.0.0.2"), "{dumped}");
    }

    #[test]
    fn device_matching() {
        let devices = cloud::device_list(&FakeCloud::new()).unwrap();
        assert_eq!(find_device(&devices, "1002").unwrap().name, "Desk Plug");
        assert_eq!(find_device(&devices, "heater").unwrap().did, "1001");
        assert!(find_device(&devices, "e").is_err(), "ambiguous part of a name");
        assert!(find_device(&devices, "kitchen").is_err());
    }

    #[test]
    fn status_reads_values_and_skips_device_information() {
        let cloud = FakeCloud::new();
        let out = status(&cloud, &FixtureSpecs, "Desk Plug", &[]).unwrap();
        assert!(out.text.contains("2-2  heater.target-temperature  = 21°C"), "{}", out.text);
        assert!(out.text.contains("= 2 (Eco, 节能)"), "{}", out.text);
        assert!(!out.text.contains("serial-number"));
        assert!(out.text.starts_with("Desk Plug | demo.plug.p1 | online | firmware 1.0.9\n"), "{}", out.text);
        assert_eq!(out.json["firmware"], "1.0.9");
        assert!(out.text.contains("write-only:") && out.text.contains("actions:"));
        assert_eq!(out.json["write_only"][0]["id"], "2-7");
        // `label` is what the text output shows: description plus the Chinese comment, or either alone.
        let on = &out.json["properties"][0];
        assert_eq!((on["comment"].as_str(), on["label"].as_str()), (Some("开关"), Some("Switch Status (开关)")));
        assert_eq!(out.json["write_only"][0]["label"], "Alarm");
        assert_eq!(out.json["actions"][0]["label"], "Set Timer");
        assert_eq!(out.json["actions"][0]["name"], "heater.set-timer");
        assert_eq!(out.json["actions"][0]["in"][0]["id"], "2-4");
        let one = status(&cloud, &FixtureSpecs, "Desk Plug", &["2-2".into()]).unwrap();
        assert_eq!(one.json["properties"].as_array().unwrap().len(), 1);
        assert!(one.json.get("actions").is_none(), "named properties only");
        let err = status(&cloud, &FixtureSpecs, "Desk Plug", &["heater.alarm".into()]).unwrap_err().to_string();
        assert!(err.contains("write-only"), "{err}");
        let err = status(&cloud, &FixtureSpecs, "Desk Plug", &["status-code".into()]).unwrap_err().to_string();
        assert!(err.contains("no read access"), "{err}");
        assert!(cloud.writes().is_empty(), "status must never write");
    }

    #[test]
    fn high_risk_writes_need_confirmation() {
        let cloud = FakeCloud::new();
        let err = set(&cloud, &FixtureSpecs, "Study Heater", "heater.on", "true", false, &FAST).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::EXIT_NEEDS_CONFIRMATION);
        assert!(err.to_string().contains("--confirmed"));
        let err = action(&cloud, &FixtureSpecs, "Study Heater", "toggle", &[], false).unwrap_err();
        assert_eq!(err.exit_code(), crate::error::EXIT_NEEDS_CONFIRMATION);
        assert!(cloud.writes().is_empty(), "nothing may be sent before confirmation");

        let out = set(&cloud, &FixtureSpecs, "Study Heater", "heater.on", "true", true, &FAST).unwrap();
        assert_eq!(out.json["applied"], json!(true));
        assert_eq!(cloud.writes(), vec![json!({"params": [{"did": "1001", "siid": 2, "piid": 1, "value": true}]})]);
    }

    #[test]
    fn set_validates_before_sending_and_reports_readback() {
        let cloud = FakeCloud { applies_writes: false, ..FakeCloud::new() };
        assert!(set(&cloud, &FixtureSpecs, "Desk Plug", "2-5", "20", false, &FAST).is_err(), "read-only");
        assert!(set(&cloud, &FixtureSpecs, "Desk Plug", "2-2", "99", false, &FAST).is_err(), "out of range");
        assert!(cloud.writes().is_empty());
        let out = set(&cloud, &FixtureSpecs, "Desk Plug", "heater.on", "on", false, &FAST).unwrap();
        assert_eq!(out.json["applied"], json!(false));
        assert!(out.text.contains("Not updated yet"), "{}", out.text);
    }

    #[test]
    fn an_interrupted_set_is_checked_and_sent_once_more() {
        // The write was lost: the read shows the old value, so it goes out a second time.
        let cloud = FakeCloud::breaking(&[false]);
        let out = set(&cloud, &FixtureSpecs, "Desk Plug", "heater.on", "true", false, &FAST).unwrap();
        assert_eq!(cloud.writes().len(), 2);
        assert_eq!((out.json["resent"].clone(), out.json["applied"].clone()), (json!(true), json!(true)));
        assert!(out.text.contains("(done); sent twice"), "{}", out.text);

        // Only the reply was lost: the read shows the new value, so nothing more is sent.
        let cloud = FakeCloud::breaking(&[true]);
        let out = set(&cloud, &FixtureSpecs, "Desk Plug", "heater.on", "true", false, &FAST).unwrap();
        assert_eq!(cloud.writes().len(), 1);
        assert_eq!((out.json["code"].clone(), out.json["result"].clone()), (Value::Null, json!("already set")));
        assert_eq!((out.json["resent"].clone(), out.json["applied"].clone()), (json!(false), json!(true)));

        // Broken twice: give up with the advice to check first.
        let cloud = FakeCloud::breaking(&[false, false]);
        let err = set(&cloud, &FixtureSpecs, "Desk Plug", "heater.on", "true", false, &FAST).unwrap_err();
        assert!(err.to_string().contains("may or may not have reached"), "{err}");
        assert_eq!(cloud.writes().len(), 2, "only one more try");

        // Write-only properties cannot be checked and actions may not be safe to repeat:
        // neither is ever sent twice.
        let cloud = FakeCloud::breaking(&[false, false]);
        assert!(set(&cloud, &FixtureSpecs, "Desk Plug", "heater.alarm", "true", false, &FAST).is_err());
        assert!(action(&cloud, &FixtureSpecs, "Desk Plug", "toggle", &[], false).is_err());
        assert_eq!(cloud.writes().len(), 2);
    }

    #[test]
    fn offline_devices_get_nothing() {
        let cloud = FakeCloud { heater_online: false, ..FakeCloud::new() };
        let err = set(&cloud, &FixtureSpecs, "Study Heater", "heater.on", "true", true, &FAST).unwrap_err();
        assert!(err.to_string().contains("offline"));
        assert!(cloud.writes().is_empty());
    }

    #[test]
    fn action_checks_arguments_and_shows_output() {
        let cloud = FakeCloud::new();
        assert!(action(&cloud, &FixtureSpecs, "Desk Plug", "set-timer", &[], false).is_err());
        assert!(action(&cloud, &FixtureSpecs, "Desk Plug", "set-timer", &["999".into()], false).is_err());
        assert!(cloud.writes().is_empty());
        let out = action(&cloud, &FixtureSpecs, "Desk Plug", "2-1", &["30".into()], false).unwrap();
        assert!(out.text.contains("returned countdown-time=30 minutes"), "{}", out.text);
        assert_eq!(cloud.writes(), vec![json!({"params": {"did": "1002", "siid": 2, "aiid": 1, "in": [30]}})]);
    }
}
