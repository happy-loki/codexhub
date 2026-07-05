use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Gui,
    Daemon,
    On,
    Off,
    Status,
    ConfigureCodexApp {
        codex_home: Option<PathBuf>,
        provider_name: Option<String>,
        provider_base_url: Option<String>,
        provider_key: Option<String>,
        model: Option<String>,
    },
    UninstallCodexApp {
        codex_home: Option<PathBuf>,
    },
}

#[derive(Debug, Clone)]
pub struct Cli {
    pub config_path: Option<PathBuf>,
    pub command: Command,
}

impl Cli {
    pub fn parse() -> anyhow::Result<Self> {
        let mut config_path = None;
        let mut remaining = Vec::new();
        let args = std::env::args().skip(1).collect::<Vec<_>>();
        let mut index = 0;

        while index < args.len() {
            match args[index].as_str() {
                "--config" | "-c" => {
                    let Some(path) = args.get(index + 1) else {
                        anyhow::bail!("{} requires a path", args[index]);
                    };
                    config_path = Some(PathBuf::from(path));
                    index += 2;
                }
                _ => {
                    remaining.extend_from_slice(&args[index..]);
                    break;
                }
            }
        }

        let command = match remaining.first().map(String::as_str) {
            None => default_command(),
            Some("gui") => Command::Gui,
            Some("daemon") | Some("run") => Command::Daemon,
            Some("on") => Command::On,
            Some("off") => Command::Off,
            Some("status") => Command::Status,
            Some("configure-codex-app") => parse_configure_codex_app(&remaining[1..])?,
            Some("uninstall-codex-app") => parse_uninstall_codex_app(&remaining[1..])?,
            Some("install-shim") | Some("uninstall-shim") | Some("shim") => anyhow::bail!(
                "CLI shim support has been removed. Use `codex-remote-gateway configure-codex-app` and Codex App remote-control instead."
            ),
            Some("-h") | Some("--help") | Some("help") => {
                print_help();
                std::process::exit(0);
            }
            Some(other) => {
                anyhow::bail!("unknown command `{other}`. Run `codex-remote-gateway help`.")
            }
        };

        Ok(Self {
            config_path,
            command,
        })
    }
}

fn default_command() -> Command {
    if cfg!(feature = "gui") {
        Command::Gui
    } else {
        Command::Daemon
    }
}

fn parse_configure_codex_app(args: &[String]) -> anyhow::Result<Command> {
    let mut codex_home = None;
    let mut provider_name = None;
    let mut provider_base_url = None;
    let mut provider_key = None;
    let mut model = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--codex-home" => {
                let Some(value) = iter.next() else {
                    anyhow::bail!("--codex-home requires a path");
                };
                codex_home = Some(PathBuf::from(value));
            }
            "--provider-name" => {
                let Some(value) = iter.next() else {
                    anyhow::bail!("--provider-name requires a name");
                };
                provider_name = Some(value.to_string());
            }
            "--provider-base-url" => {
                let Some(value) = iter.next() else {
                    anyhow::bail!("--provider-base-url requires a URL");
                };
                provider_base_url = Some(value.to_string());
            }
            "--provider-key" => {
                let Some(value) = iter.next() else {
                    anyhow::bail!("--provider-key requires a token");
                };
                provider_key = Some(value.to_string());
            }
            "--model" => {
                let Some(value) = iter.next() else {
                    anyhow::bail!("--model requires a model name");
                };
                model = Some(value.to_string());
            }
            other => anyhow::bail!("unknown configure-codex-app argument `{other}`"),
        }
    }
    Ok(Command::ConfigureCodexApp {
        codex_home,
        provider_name,
        provider_base_url,
        provider_key,
        model,
    })
}

fn parse_uninstall_codex_app(args: &[String]) -> anyhow::Result<Command> {
    let mut codex_home = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--codex-home" => {
                let Some(value) = iter.next() else {
                    anyhow::bail!("--codex-home requires a path");
                };
                codex_home = Some(PathBuf::from(value));
            }
            other => anyhow::bail!("unknown uninstall-codex-app argument `{other}`"),
        }
    }
    Ok(Command::UninstallCodexApp { codex_home })
}

pub fn print_help() {
    println!(
        r#"codex-remote-gateway

Usage:
  codex-remote-gateway [--config PATH] gui
  codex-remote-gateway [--config PATH] daemon
  codex-remote-gateway [--config PATH] on
  codex-remote-gateway [--config PATH] off
  codex-remote-gateway [--config PATH] status
  codex-remote-gateway [--config PATH] configure-codex-app [--codex-home PATH] [--provider-name NAME] [--provider-base-url URL] [--provider-key TOKEN] [--model MODEL]
  codex-remote-gateway [--config PATH] uninstall-codex-app [--codex-home PATH]

Default command is gui when built with the gui feature, otherwise daemon.
"#
    );
}
