use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "ptun", version, about)]
pub struct Cli {
    #[arg(short, long, global = true)]
    pub config: Option<PathBuf>,

    #[arg(long, global = true)]
    pub no_config: bool,

    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[arg(long, global = true)]
    pub log_level: Option<String>,

    #[arg(short, long, global = true)]
    pub quiet: bool,

    #[arg(long, global = true, conflicts_with = "quiet")]
    pub no_quiet: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Launch a command and route its traffic through a proxy.
    Run(RunArgs),
    /// Validate config, permissions, TUN support, and proxy reachability.
    Check,
    /// Show currently tracked ptun sessions.
    Status,
    /// Run deeper environment diagnostics.
    Doctor,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    #[arg(short, long)]
    pub proxy: Option<String>,

    #[arg(short, long)]
    pub dns: Option<String>,

    #[arg(short = 't', long)]
    pub tun_name: Option<String>,

    #[arg(long)]
    pub mtu: Option<u16>,

    #[arg(short = '6', long)]
    pub ipv6: bool,

    #[arg(long, conflicts_with = "ipv6")]
    pub no_ipv6: bool,

    #[arg(short, long, value_enum)]
    pub udp: Option<UdpMode>,

    #[arg(long)]
    pub fail_open: bool,

    #[arg(long, conflicts_with = "fail_open")]
    pub fail_closed: bool,

    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true, num_args = 1..)]
    pub command: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UdpMode {
    Auto,
    On,
    Off,
}
