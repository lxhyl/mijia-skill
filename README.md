# mijia-skill

**English** | [简体中文](README.zh-CN.md)

Let AI agents read and control **Xiaomi Mi Home (Mijia, 米家)** devices.

- **`mijia`**: a single-binary CLI (Rust) that talks to the Xiaomi cloud MiOT API. Any agent that
  can run shell commands can use it, and so can you.
- **An [Agent Skill](https://agentskills.io)** (`skills/mijia/SKILL.md`) that teaches agents how to
  use it safely. Works with Claude Code, Codex and other agents that load `SKILL.md` skills.

It is not tied to particular models. For each device the CLI fetches the model's public
[MiOT spec](https://home.miot-spec.com/) (properties, actions, formats, allowed values), caches it,
and checks every write against it. The agent looks up what a device supports before touching it.

> Unofficial project, not affiliated with Xiaomi. It uses the same undocumented cloud API as the
> Mi Home app, which may change without notice.

## Safety and privacy by design

- **The agent never logs in.** Only the account owner runs `mijia login`, in their own terminal; it
  refuses to run without an interactive terminal. Login is by QR code: the CLI never sees your
  password or SMS codes.
- **The login cache stays put.** It is written with mode `600` to `~/.config/mijia/token.json`,
  read-only for every other command, and never printed.
- **Minimal data.** `list` keeps only name, model, did and online state from the cloud's device list
  (which also contains device keys and IPs). `status` skips serial numbers and firmware details
  unless asked.
- **High-risk devices need explicit confirmation.** Writes to locks, doors and gates, gas and
  valves, anything that heats, cameras, and routers/gateways are refused unless the agent passes
  `--confirmed`, which the skill allows only after the user approved that exact operation. The
  category comes from the device's spec type, not its (user-editable) name.
- **Validated writes.** Values are checked against the spec (type, range, step, allowed values)
  before anything is sent; offline devices get nothing; `set` reads the value back.
- **No telemetry.** The CLI talks only to `*.api.io.mi.com`, `account.xiaomi.com` (login) and
  `miot-spec.org` (public specs).

## Install the CLI

**Prebuilt binary** (macOS arm64/x86_64, Linux x86_64/arm64, Windows x86_64): download the archive
for your platform from the [Releases](https://github.com/OWNER/mijia-skill/releases) page, check it
against the `.sha256` file next to it, and put `mijia` on your `PATH`:

```sh
shasum -a 256 -c mijia-v0.1.0-aarch64-apple-darwin.tar.gz.sha256
tar xzf mijia-v0.1.0-aarch64-apple-darwin.tar.gz
install -m 755 mijia-v0.1.0-aarch64-apple-darwin/mijia ~/.local/bin/
```

On macOS, if a browser download is blocked by Gatekeeper, run
`xattr -d com.apple.quarantine ~/.local/bin/mijia`.

**With Cargo** (Rust 1.85 or newer):

```sh
cargo install --locked --git https://github.com/OWNER/mijia-skill
# or from a clone:
cargo install --locked --path .
```

Check it: `mijia --version`, then `mijia spec --model zhimi.airpurifier.mb3` (works without a login).

## Log in (account owner only)

Run this **yourself, in your own terminal** (not through an agent, and not with an agent's
`!` shell prefix):

```sh
mijia login                 # mainland China accounts
mijia login --region de     # other regions: de, i2, ru, sg, us, tw, in
```

A QR code opens (or use the printed link to show it in any browser). In the Mi Home app tap
**+ → Scan**, scan it, and confirm on the phone. The login cache is saved to
`~/.config/mijia/token.json` (mode `600`). It grants full access to your Xiaomi account: never
share it or commit it. If the device list comes back empty, `mijia login` checks the other regions
and remembers the one with your devices. When the login expires, commands exit with code 4 and the
agent will ask you to run `mijia login` again.

Your region is shown in the Mi Home app under Profile → Settings → Region.

## Install the skill

```sh
# Any agent supported by the skills CLI (https://github.com/vercel-labs/skills)
npx skills add OWNER/mijia-skill

# Claude Code (user-wide)
mkdir -p ~/.claude/skills && cp -R skills/mijia ~/.claude/skills/

# Codex (user-wide)
mkdir -p ~/.agents/skills && cp -R skills/mijia ~/.agents/skills/
```

For a single project use `.claude/skills/` or `.agents/skills/` in the repository instead. For other
agents, point them at `skills/mijia/SKILL.md`, or paste its contents into the agent's instructions
(for example `AGENTS.md`).

### Recommended permissions (Claude Code)

Let read-only commands run freely, keep writes behind a prompt, and block the login and the token
file. In `.claude/settings.json` or `~/.claude/settings.json`:

```json
{
  "permissions": {
    "allow": ["Bash(mijia list:*)", "Bash(mijia status:*)", "Bash(mijia spec:*)", "Bash(mijia doctor:*)"],
    "ask": ["Bash(mijia set:*)", "Bash(mijia action:*)"],
    "deny": ["Bash(mijia login:*)", "Read(~/.config/mijia/**)"]
  }
}
```

`Read(...)` rules only cover Claude's file tools, not `cat` in a shell; enable Claude Code's
sandbox if you need that enforced at the OS level. Other agents have similar settings; the idea is
the same.

## Usage

```text
$ mijia list
Bedroom Humidifier | deerma.humidifier.jsq5 | did 123456789 | online
Desk Plug | chuangmi.plug.212a01 | did 234567890 | online
Living Room Purifier | zhimi.airpurifier.mb3 | did 345678901 | online
Balcony Thermometer | cgllc.sensor_ht.qpg1 | did blt.3.abcdef | online state unknown

$ mijia status "Desk Plug"
$ mijia spec "Desk Plug"
$ mijia set "Bedroom Humidifier" humidifier.target-humidity 60
$ mijia action "Living Room Purifier" filter.reset-filter-life
$ mijia status 123456789 environment.relative-humidity --json
```

`<device>` is a name, a unique part of a name, or a did. Properties and actions can be written as
`siid-iid` (`2-1`), `service.name` (`switch.on`) or a bare name (`on`). Add `--json` for
machine-readable output. The full reference, including JSON shapes and exit codes, is in
[skills/mijia/references/cli.md](skills/mijia/references/cli.md).

| Exit code | Meaning |
|---|---|
| 0 | ok |
| 1 | error |
| 2 | bad usage |
| 3 | high-risk device: ask the user, then add `--confirmed` |
| 4 | not logged in or login expired: the owner runs `mijia login` |

## Limitations

- Only **MiOT** devices (the large majority of current Mi Home devices). Older miIO-only devices
  without a MiOT spec, infrared remotes and third-party integrations are not supported.
- Cloud only: it needs internet access, and values written can take a few seconds to show up.
- Bluetooth devices report through a gateway or phone, so their values may be stale.
- QR login only. Password + SMS login is deliberately not implemented, so the CLI never handles
  passwords.
- No MCP server yet. Agents that can run shell commands can use the CLI directly; a thin MCP wrapper
  can be added later for agents that cannot.

## Development

```sh
cargo test          # unit tests, no network or Xiaomi account needed
cargo clippy --all-targets
```

Tests use a synthetic spec and a fake cloud, so no test touches a real device. Never commit a login
cache (`.gitignore` excludes the usual names).

## Acknowledgements

The cloud protocol details (request signing, QR login flow) were learned from
[MiService](https://github.com/Yonsm/MiService) (MIT) and
[Xiaomi-cloud-tokens-extractor](https://github.com/PiotrMachowski/Xiaomi-cloud-tokens-extractor)
(MIT); this code is an independent implementation. Device specs come from
[miot-spec.org](https://miot-spec.org).

## License

[MIT](LICENSE)
