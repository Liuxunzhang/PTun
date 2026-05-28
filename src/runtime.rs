use anyhow::{Context, anyhow};

use crate::{cli::RunArgs, config::EffectiveConfig, diagnostics, linux};

pub fn run(args: &RunArgs, config: &EffectiveConfig) -> anyhow::Result<()> {
    diagnostics::validate_config(config)?;
    linux::EnvironmentReport::collect().validate_for_run()?;

    Err(anyhow!(
        "transparent proxy data plane is not implemented yet; parsed command was: {}",
        args.command.join(" ")
    ))
    .context("ptun run setup checks passed")
}
