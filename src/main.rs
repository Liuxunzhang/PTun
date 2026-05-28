mod cli;
mod config;
mod diagnostics;
mod dns_cache;
mod linux;
mod packet;
mod proxy;
mod runtime;

use anyhow::Context;
use clap::Parser;
use cli::{Cli, Command};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = config::EffectiveConfig::load(&cli).context("failed to load configuration")?;
    init_logging(&config);

    match &cli.command {
        Command::Run(run) => runtime::run(run, &config),
        Command::Check => diagnostics::check(&config),
        Command::Status => diagnostics::status(&config),
        Command::Doctor => diagnostics::doctor(&config),
    }
}

fn init_logging(config: &config::EffectiveConfig) {
    if config.quiet {
        return;
    }

    let filter = config
        .log_level
        .as_deref()
        .unwrap_or("warn")
        .parse::<tracing_subscriber::EnvFilter>()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .without_time()
        .try_init();
}
