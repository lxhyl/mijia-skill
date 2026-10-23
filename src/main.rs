mod cloud;
mod commands;
mod config;
mod error;
mod login;
mod spec;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ArgGroup, Parser, Subcommand};

use crate::cloud::Cloud;
use crate::config::Paths;
use crate::error::Result;
use crate::spec::SpecStore;

/// Read and control Xiaomi Mi Home (Mijia) devices through the Xiaomi cloud (MiOT).
#[derive(Parser)]
#[command(name = "mijia", version, after_help = AFTER_HELP)]
struct Cli {
    /// Login cache to use [default: ~/.config/mijia/token.json, or $MIJIA_TOKEN]
    #[arg(long, global = true, value_name = "PATH")]
    token: Option<PathBuf>,
    /// Xiaomi cloud region: cn, de, i2, ru, sg, us, tw, in [default: $MIJIA_REGION, else the one saved by `mijia login`, else cn]
    #[arg(long, global = true)]
    region: Option<String>,
    /// Print JSON instead of text
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Cmd,
}

const AFTER_HELP: &str = "\
<device> is a device name, a unique part of one, or a did (see `mijia list`).
<property>/<action> is siid-iid (2-1), service.name (switch.on) or a bare name (on); see `mijia spec`.

Exit codes: 0 ok, 1 error, 2 bad usage, 3 high-risk device: ask the user, then add --confirmed,
4 not logged in or login expired: the account owner must run `mijia login`.";

#[derive(Subcommand)]
enum Cmd {
    /// List the account's devices: name, model, did, online state
    List,
    /// Show what a device or model supports: properties and actions, with formats and allowed values
    #[command(group(ArgGroup::new("target").required(true).args(["device", "model"])))]
    Spec {
        device: Option<String>,
        /// Look up a model (e.g. zhimi.airpurifier.mb3) without logging in
        #[arg(long)]
        model: Option<String>,
    },
    /// Read current values: every readable property, or only the ones named
    Status { device: String, properties: Vec<String> },
    /// Write one property, then read it back
    Set {
        device: String,
        property: String,
        #[arg(allow_hyphen_values = true)]
        value: String,
        /// The user explicitly approved this change (required for locks, gas, heating, cameras, network gear)
        #[arg(long)]
        confirmed: bool,
    },
    /// Run an action, with its arguments in order
    Action {
        device: String,
        action: String,
        #[arg(allow_hyphen_values = true)]
        args: Vec<String>,
        /// The user explicitly approved this action (required for locks, gas, heating, cameras, network gear)
        #[arg(long)]
        confirmed: bool,
    },
    /// Log in by scanning a QR code with the Mi Home app. For the account owner only, in their own terminal
    Login,
    /// Show where the login cache, config and spec cache are (never prints credentials)
    Doctor,
}

fn run(cli: Cli) -> Result<String> {
    let paths = Paths::resolve(cli.token)?;
    let specs = SpecStore::new(&paths.cache);
    let cloud = || -> Result<Cloud> {
        let (region, _) = config::resolve_region(cli.region.clone(), &paths)?;
        Cloud::new(config::load_token(&paths.token)?, &paths.token, &region)
    };
    let output = match cli.command {
        Cmd::Login => {
            login::run(&paths, cli.region)?;
            return Ok(String::new());
        }
        Cmd::Doctor => return doctor(&paths, cli.region),
        Cmd::List => commands::list(&cloud()?)?,
        Cmd::Spec { model: Some(model), .. } => commands::spec_of_model(&specs, &model)?,
        Cmd::Spec { device, .. } => commands::spec_of_device(&cloud()?, &specs, &device.unwrap_or_default())?,
        Cmd::Status { device, properties } => commands::status(&cloud()?, &specs, &device, &properties)?,
        Cmd::Set { device, property, value, confirmed } => {
            commands::set(&cloud()?, &specs, &device, &property, &value, confirmed, &Default::default())?
        }
        Cmd::Action { device, action, args, confirmed } => {
            commands::action(&cloud()?, &specs, &device, &action, &args, confirmed)?
        }
    };
    Ok(if cli.json { serde_json::to_string_pretty(&output.json)? } else { output.text })
}

fn doctor(paths: &Paths, region_flag: Option<String>) -> Result<String> {
    let (region, source) = config::resolve_region(region_flag, paths)?;
    let token = match std::fs::metadata(&paths.token) {
        Err(_) => "missing: the account owner must run `mijia login`".to_string(),
        #[cfg(unix)]
        Ok(meta) => {
            use std::os::unix::fs::PermissionsExt;
            let mode = meta.permissions().mode() & 0o777;
            let warning = if mode & 0o077 != 0 { " (too open: run chmod 600 on it)" } else { "" };
            format!("present, mode {mode:o}{warning}")
        }
        #[cfg(not(unix))]
        Ok(_) => "present".to_string(),
    };
    Ok(format!(
        "mijia {}\nlogin cache: {} ({token})\nregion:      {region} (from {source})\nconfig:      {}\nspec cache:  {}\n\
         Run `mijia list` to check that the login works.",
        env!("CARGO_PKG_VERSION"),
        paths.token.display(),
        paths.config.display(),
        paths.cache.display(),
    ))
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(text) => {
            if !text.is_empty() {
                println!("{text}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("mijia: {e}");
            ExitCode::from(e.exit_code() as u8)
        }
    }
}
