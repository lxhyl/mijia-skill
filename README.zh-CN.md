# mijia-skill

[English](README.md) | **简体中文**

让 AI agent 读取和控制**米家**设备。

- **`mijia`**：单个二进制文件的命令行工具（Rust），走小米云的 MiOT 接口。任何能跑 shell 命令的
  agent 都能用，人也能直接用。
- **一个 [Agent Skill](https://agentskills.io)**（`skills/mijia/SKILL.md`）：教 agent 怎么安全地用它。
  Claude Code、Codex 以及其他支持 `SKILL.md` 的 agent 都能装。

不绑定具体型号。每台设备都按型号去拉公开的 [MiOT spec](https://home.miot-spec.com/)
（属性、动作、格式、可选值），缓存到本地，每次写之前都按 spec 校验。agent 先查清楚设备支持什么，再去操作。

> 非官方项目，与小米无关。用的是米家 App 同款的非公开云端接口，随时可能变动。

## 安全与隐私

- **agent 不登录。** 只有账号主人本人在自己的终端里跑 `mijia login`；没有交互式终端时它直接拒绝运行。
  登录用扫码，CLI 从不经手密码和短信验证码。
- **登录缓存不外流。** 以 `600` 权限写到 `~/.config/mijia/token.json`，其他命令只读不写，从不打印。
- **只取必要的数据。** 小米云返回的设备列表里还有设备密钥和 IP，`list` 只保留名称、型号、did、在线状态；
  `status` 默认不读序列号，设备信息里只显示固件版本。
- **高风险设备要明确确认。** 对门锁/门/闸门、燃气和阀门、加热类、摄像头、路由器/网关的写操作，
  不带 `--confirmed` 一律拒绝；skill 规定只有用户明确同意这一次具体操作后才能加。类别按设备 spec 的类型判断，
  不看用户能随便改的设备名。
- **写之前先校验。** 按 spec 检查类型、范围、步长、可选值，不合法就不发；离线设备不发；`set` 写完会读回确认。
- **没有遥测。** 只连 `*.api.io.mi.com`、`account.xiaomi.com`（登录）和 `miot-spec.org`（公开 spec）。

## 安装 CLI

**预编译包**（macOS arm64/x86_64、Linux x86_64/arm64、Windows x86_64）：在
[Releases](https://github.com/lxhyl/mijia-skill/releases) 页面下载对应平台的压缩包，用旁边的 `.sha256`
文件校验，然后把 `mijia` 放进 `PATH`：

```sh
shasum -a 256 -c mijia-v0.1.0-aarch64-apple-darwin.tar.gz.sha256
tar xzf mijia-v0.1.0-aarch64-apple-darwin.tar.gz
install -m 755 mijia-v0.1.0-aarch64-apple-darwin/mijia ~/.local/bin/
```

macOS 上用浏览器下载的文件如果被 Gatekeeper 拦住，运行
`xattr -d com.apple.quarantine ~/.local/bin/mijia`。

**用 Cargo 装**（Rust 1.85 及以上）：

```sh
cargo install --locked --git https://github.com/lxhyl/mijia-skill
# 或者在克隆下来的目录里：
cargo install --locked --path .
```

验证：`mijia --version`，再跑 `mijia spec --model zhimi.airpurifier.mb3`（不用登录）。

## 登录（只能账号主人本人做）

**自己在自己的终端里**运行（不要让 agent 跑，也不要用 agent 的 `!` 前缀跑）：

```sh
mijia login                 # 中国大陆账号
mijia login --region de     # 其他地区：de、i2、ru、sg、us、tw、in
```

会弹出二维码（也可以用打印出来的链接在任意浏览器里显示）。在米家 App 右上角 **+ → 扫一扫**，
扫码后在手机上确认。登录缓存存到 `~/.config/mijia/token.json`（权限 `600`），它等于整个小米账号的
访问权限：不要发给任何人，也不要提交到仓库。如果设备列表是空的，`mijia login` 会去查其他地区，
并记住有设备的那个。登录过期后命令会以退出码 4 结束，agent 会请你重新跑 `mijia login`。

地区在米家 App「我的 → 设置 → 地区」里看。

## 安装 skill

```sh
# skills CLI 支持的任意 agent（https://github.com/vercel-labs/skills）
npx skills add lxhyl/mijia-skill

# Claude Code（当前用户）
mkdir -p ~/.claude/skills && cp -R skills/mijia ~/.claude/skills/

# Codex（当前用户）
mkdir -p ~/.agents/skills && cp -R skills/mijia ~/.agents/skills/
```

只给某个项目用的话，放到仓库里的 `.claude/skills/` 或 `.agents/skills/`。其他 agent：让它读
`skills/mijia/SKILL.md`，或者把内容贴进它的说明文件（比如 `AGENTS.md`）。

### 建议的权限设置（Claude Code）

只读命令直接放行，写操作每次询问，禁止登录和读取登录缓存。写在 `.claude/settings.json`
或 `~/.claude/settings.json`：

```json
{
  "permissions": {
    "allow": ["Bash(mijia list:*)", "Bash(mijia status:*)", "Bash(mijia spec:*)", "Bash(mijia doctor:*)"],
    "ask": ["Bash(mijia set:*)", "Bash(mijia action:*)"],
    "deny": ["Bash(mijia login:*)", "Read(~/.config/mijia/**)"]
  }
}
```

`Read(...)` 规则只管 Claude 自带的读文件工具，管不到 shell 里的 `cat`；要在系统层面拦住，需要开启
Claude Code 的 sandbox。其他 agent 也有类似的设置，思路一样。

## 用法

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

`<设备>` 写名称、名称里能唯一对上的一部分，或者 did。属性和动作可以写 `siid-iid`（`2-1`）、
`服务.名称`（`switch.on`）或单独的名称（`on`）。加 `--json` 输出机器可读的格式。完整说明（含 JSON 结构、
退出码）见 [skills/mijia/references/cli.md](skills/mijia/references/cli.md)。

| 退出码 | 含义 |
|---|---|
| 0 | 成功 |
| 1 | 出错 |
| 2 | 用法不对 |
| 3 | 高风险设备：先问用户，同意后加 `--confirmed` |
| 4 | 没登录或登录过期：账号主人跑 `mijia login` |

## 局限

- 只支持 **MiOT** 设备（现在的米家设备绝大多数都是）。没有 MiOT spec 的老 miIO 设备、红外遥控、第三方接入不支持。
- 只走云端：要联网，写完要过几秒才能读到新值。
- 蓝牙设备靠网关或手机上报，读到的值可能不是最新的。
- 只支持扫码登录。故意没做「密码 + 短信」登录，这样 CLI 永远不经手密码。
- 暂时没有 MCP server。能跑 shell 的 agent 直接用 CLI 即可；以后可以为不能跑 shell 的 agent 加一层很薄的 MCP 封装。

## 开发

```sh
cargo test          # 单元测试，不联网，不需要小米账号
cargo clippy --all-targets
```

测试用的是虚构的 spec 和假的云端，不会碰任何真实设备。不要提交登录缓存（`.gitignore` 已排除常见文件名）。

## 致谢

云端协议的细节（请求签名、扫码登录流程）参考了 [MiService](https://github.com/Yonsm/MiService)（MIT）和
[Xiaomi-cloud-tokens-extractor](https://github.com/PiotrMachowski/Xiaomi-cloud-tokens-extractor)（MIT），
本项目的代码是独立实现的。设备 spec 来自 [miot-spec.org](https://miot-spec.org)。

## 许可证

[MIT](LICENSE)
