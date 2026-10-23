# `mijia` CLI reference

## Commands

| Command | What it does | Login needed |
|---|---|---|
| `mijia list` | Devices on the account: `name \| model \| did \| online state` | yes |
| `mijia spec <device>` | The device's MiOT spec: every property (`siid-piid`, access, allowed values) and action (`siid-aiid`, arguments) | yes |
| `mijia spec --model <model>` | The same for a model, e.g. `zhimi.airpurifier.mb3` | no |
| `mijia status <device> [property...]` | Current values of all readable properties (or only the named ones), with write hints and the list of actions | yes |
| `mijia set <device> <property> <value> [--confirmed]` | Write one property, then read it back | yes |
| `mijia action <device> <action> [args...] [--confirmed]` | Run an action; arguments in the order `spec` lists them | yes |
| `mijia doctor` | Paths of the login cache, config and spec cache, and the region in use. Never prints credentials | no |
| `mijia login [--region R]` | QR-code login. **Only the account owner runs this, in their own terminal.** It refuses to run without an interactive terminal | n/a |

Global options (any position): `--json`, `--region <cn|de|i2|ru|sg|us|tw|in>`, `--token <path>`.

`status` leaves out the `device-information` service (manufacturer, firmware, serial number)
unless you name one of its properties.

Text output truncates very long values (for example a router's client list); use `--json`
for the full value.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Error (device not found, invalid value, device offline, rejected by the device, network) |
| 2 | Bad command-line usage |
| 3 | High-risk device: nothing was sent. Ask the user, then rerun with `--confirmed` |
| 4 | Not logged in or login expired: the account owner must run `mijia login` |

## Values

- `bool`: `true`/`false` (`on`/`off`, `1`/`0`, `yes`/`no`, `开`/`关` also work).
- Value lists (`1=Level1, 2=Level2`): the number, the English description (`Level1`) or the
  Chinese comment shown in parentheses.
- Ranges (`16..32 step 0.5°C`): a number inside the range and on the step. Integer formats reject
  fractions.
- Strings: passed as given. Quote arguments that contain spaces.
- Negative numbers work as values (`mijia set dev 2-3 -5`).

## MiOT result codes

`set` and `action` report `done` (code 0) or `accepted` (code 1: the device applies it
asynchronously, so the read-back can lag). Any other code is an error from the device or cloud,
for example when the device is busy, in a state that does not allow the change, or offline.

## JSON output (`--json`)

- `list`: `[{"name", "model", "did", "online"}]` (`online` may be `null` for Bluetooth devices).
- `spec`: `{"device", "model", "type", "kind", "risk", "properties": [{"id", "name", "description",
  "comment", "format", "access", "unit", "range", "values", "hint"}], "actions": [{"id", "name",
  "description", "comment", "in": [{"id", "name", "hint"}], "out": [...]}]}`.
- `status`: `{"device", "kind", "risk", "properties": [{"id", "name", "description", "value",
  "display", "writable", "hint"}], "write_only": [{"id", "name", "description", "hint"}],
  "actions": [...]}` (`actions` as in `spec`). `value` is `null` when the device did not return it.
  `write_only` and `actions` are only present when no properties are named.
- `set`: `{"device", "property", "name", "value", "display", "code", "result", "read_back",
  "applied"}`. `applied` is `true`/`false` after the read-back, `null` for write-only properties.
- `action`: `{"device", "action", "name", "args", "code", "result", "out": [{"id", "name", "value",
  "display"}]}`.

`risk` is `null` or one of `lock / access`, `gas / valve`, `heating`, `camera / privacy`,
`network`. It is derived from the device's spec type, not its name.

## Files and environment

| What | Default | Override |
|---|---|---|
| Login cache (secret) | `~/.config/mijia/token.json` (`%APPDATA%\mijia\token.json` on Windows) | `--token`, `MIJIA_TOKEN` |
| Config (region) | `~/.config/mijia/config.json` | `MIJIA_CONFIG_DIR` (directory) |
| Spec cache (public data) | `~/.cache/mijia/` (`%LOCALAPPDATA%\mijia\cache` on Windows) | `MIJIA_CACHE_DIR` |
| Region | saved by `mijia login --region`, else `cn` | `--region`, `MIJIA_REGION` |

`XDG_CONFIG_HOME` and `XDG_CACHE_HOME` are honoured. The login cache uses the same format as
MiService's `.mi.token`, so an existing MiService login can be used with `--token`.

## Troubleshooting

- **"no device matches"**: run `mijia list` and use the exact name or the did.
- **"matches several devices / properties"**: use the full name or did, or the `siid-iid` form.
- **Empty device list**: almost always the wrong region (see SKILL.md).
- **Bluetooth devices** (thermometers, scales, door sensors) report through a gateway or the phone;
  values can be stale and the online state can be unknown.
- **"cannot reach the Xiaomi cloud"**: check the network or proxy (`HTTPS_PROXY` is honoured).
- **Spec lookups** need miot-spec.org once per model; after that the cached copy is used, so they
  also work offline.
