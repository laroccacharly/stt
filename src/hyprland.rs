//! Installs the End key binding into the Hyprland config.

use std::{fs, path::PathBuf, process::Command};

use anyhow::{Context, Result, bail};

const SOURCE_LINE: &str = r#"require("hypr.stt")"#;
const BINDING: &str = "o.bind(\"END\", \"Toggle live speech to text\", \"stt toggle\")\n";

fn hypr_dir() -> Result<PathBuf> {
    Ok(std::env::home_dir()
        .context("no home directory")?
        .join(".config/hypr"))
}

pub fn install() -> Result<()> {
    let dir: PathBuf = hypr_dir()?;
    let binding: PathBuf = dir.join("stt.lua");
    fs::write(&binding, BINDING)?;
    let config_path: PathBuf = dir.join("hyprland.lua");
    let mut config: String = fs::read_to_string(&config_path)?;
    if !config.lines().any(|line| line.trim() == SOURCE_LINE) {
        if !config.is_empty() && !config.ends_with('\n') {
            config.push('\n');
        }
        config.push_str(SOURCE_LINE);
        config.push('\n');
        fs::write(&config_path, config)?;
    }
    reload()?;
    println!("Installed End binding in {}", binding.display());
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let dir: PathBuf = hypr_dir()?;
    let config_path: PathBuf = dir.join("hyprland.lua");
    let config: String = fs::read_to_string(&config_path)?;

    let kept: Vec<&str> = config
        .split('\n')
        .filter(|line| line.trim() != SOURCE_LINE)
        .collect();
    if kept.len() != config.split('\n').count() {
        fs::write(&config_path, kept.join("\n"))?;
    }
    match fs::remove_file(dir.join("stt.lua")) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    reload()?;
    println!("Removed End binding");
    Ok(())
}

fn reload() -> Result<()> {
    if !Command::new("hyprctl")
        .arg("reload")
        .stdout(std::process::Stdio::null())
        .status()?
        .success()
    {
        bail!("hyprctl reload failed");
    }
    Ok(())
}
