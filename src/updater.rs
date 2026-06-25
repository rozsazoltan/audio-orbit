use anyhow::{Context, Result};
use reqwest::blocking::Client;
use semver::Version;
use serde::Deserialize;
use std::{
    env,
    fs,
    path::PathBuf,
    process::Command,
};

const RELEASES_API: &str = "https://api.github.com/repos/rozsazoltan/audio-orbit/releases";
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/rozsazoltan/audio-orbit/releases/latest";
const USER_AGENT: &str = "Audio-Orbit-Updater";

#[derive(Clone, Debug)]
pub struct UpdateCheck {
    pub current_version: String,
    pub latest_version: String,
    pub asset_name: Option<String>,
    pub asset_download_url: Option<String>,
    pub is_update_available: bool,
    pub prerelease: bool,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

pub fn check_for_update(include_prereleases: bool) -> Result<UpdateCheck> {
    let client = Client::builder().user_agent(USER_AGENT).build()?;

    let latest_release = if include_prereleases {
        let releases: Vec<GitHubRelease> = client
            .get(RELEASES_API)
            .send()
            .context("failed to contact GitHub releases")?
            .error_for_status()
            .context("GitHub releases request failed")?
            .json()
            .context("failed to parse GitHub releases response")?;

        let mut candidates = releases
            .into_iter()
            .filter(|release| !release.draft)
            .filter_map(|release| {
                let parsed = Version::parse(release.tag_name.trim_start_matches('v')).ok()?;
                Some((parsed, release))
            })
            .collect::<Vec<_>>();

        candidates.sort_by(|left, right| right.0.cmp(&left.0));

        candidates
            .into_iter()
            .next()
            .map(|(_, release)| release)
            .context("no suitable GitHub release was found")?
    } else {
        client
            .get(LATEST_RELEASE_API)
            .send()
            .context("failed to contact GitHub latest release")?
            .error_for_status()
            .context("GitHub latest release request failed")?
            .json()
            .context("failed to parse GitHub latest release response")?
    };

    let current_version = env!("CARGO_PKG_VERSION").to_owned();
    let current_semver = Version::parse(&current_version).context("invalid current application version")?;
    let latest_semver = Version::parse(latest_release.tag_name.trim_start_matches('v'))
        .context("invalid latest GitHub release version")?;

    let asset = latest_release
        .assets
        .iter()
        .find(|asset| asset.name.ends_with("windows-x64.exe"))
        .or_else(|| latest_release.assets.iter().find(|asset| asset.name.ends_with(".exe")));

    Ok(UpdateCheck {
        current_version,
        latest_version: latest_semver.to_string(),
        asset_name: asset.map(|asset| asset.name.clone()),
        asset_download_url: asset.map(|asset| asset.browser_download_url.clone()),
        is_update_available: latest_semver > current_semver,
        prerelease: latest_release.prerelease,
    })
}

pub fn install_update(check: &UpdateCheck) -> Result<()> {
    let Some(download_url) = check.asset_download_url.as_ref() else {
        anyhow::bail!("the selected release does not contain a Windows executable asset");
    };

    let current_exe = env::current_exe().context("failed to resolve current executable path")?;
    let update_dir = current_exe
        .parent()
        .map(|parent| parent.join(".audio-orbit-data").join("update"))
        .unwrap_or_else(|| env::temp_dir().join("audio-orbit-update"));
    fs::create_dir_all(&update_dir)
        .with_context(|| format!("failed to create update folder: {}", update_dir.display()))?;

    let new_exe = update_dir.join(
        check
            .asset_name
            .clone()
            .unwrap_or_else(|| "audio-orbit-update.exe".to_owned()),
    );

    let client = Client::builder().user_agent(USER_AGENT).build()?;
    let bytes = client
        .get(download_url)
        .send()
        .context("failed to download update asset")?
        .error_for_status()
        .context("GitHub update asset download failed")?
        .bytes()
        .context("failed to read downloaded update asset")?;
    fs::write(&new_exe, &bytes)
        .with_context(|| format!("failed to write update asset: {}", new_exe.display()))?;

    launch_windows_replacer(&current_exe, &new_exe)?;
    std::process::exit(0);
}

#[cfg(windows)]
fn launch_windows_replacer(current_exe: &PathBuf, new_exe: &PathBuf) -> Result<()> {
    let update_dir = new_exe
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    let script = update_dir.join("audio-orbit-update.cmd");
    let script_contents = format!(
        "@echo off\r\n\
setlocal\r\n\
set \"NEW_EXE={new_exe}\"\r\n\
set \"CURRENT_EXE={current_exe}\"\r\n\
for /L %%i in (1,1,30) do (\r\n\
  copy /Y \"%NEW_EXE%\" \"%CURRENT_EXE%\" > nul 2>&1 && goto copied\r\n\
  timeout /t 1 /nobreak > nul\r\n\
)\r\n\
exit /b 1\r\n\
:copied\r\n\
start \"\" \"%CURRENT_EXE%\"\r\n\
del \"%NEW_EXE%\" > nul 2>&1\r\n\
del \"%~f0\"\r\n",
        new_exe = new_exe.display(),
        current_exe = current_exe.display()
    );
    fs::write(&script, script_contents)
        .with_context(|| format!("failed to write updater script: {}", script.display()))?;
    let script_string = script.to_string_lossy().to_string();
    Command::new("cmd")
        .args(["/C", "start", "", script_string.as_str()])
        .spawn()
        .context("failed to launch updater script")?;
    Ok(())
}

#[cfg(not(windows))]
fn launch_windows_replacer(_current_exe: &PathBuf, _new_exe: &PathBuf) -> Result<()> {
    anyhow::bail!("self-update replacement is currently implemented for Windows builds only")
}

pub fn open_releases_page() -> Result<()> {
    #[cfg(windows)]
    {
        Command::new("cmd")
            .args(["/C", "start", "", "https://github.com/rozsazoltan/audio-orbit/releases"])
            .spawn()
            .context("failed to open releases page")?;
    }

    #[cfg(not(windows))]
    {
        Command::new("xdg-open")
            .arg("https://github.com/rozsazoltan/audio-orbit/releases")
            .spawn()
            .context("failed to open releases page")?;
    }

    Ok(())
}

pub fn repository_label() -> &'static str {
    "rozsazoltan/audio-orbit"
}
