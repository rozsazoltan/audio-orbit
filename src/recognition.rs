use crate::config::external_tools_dir;
use anyhow::{Context, Result};
use reqwest::{blocking::Client, StatusCode};
use semver::Version;
use serde::Deserialize;
use serde_json::Value;
use std::{
    env,
    fs,
    io::{self, Cursor},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

const SONGREC_RELEASES_API: &str = "https://api.github.com/repos/marin-m/SongRec/releases";
const SONGREC_USER_AGENT: &str = "Audio-Orbit-SongRec-Manager";
const SONGREC_VERSION_FILE: &str = "songrec.version";


#[derive(Clone, Debug)]
pub struct SongRecToolStatus {
    pub installed_version: Option<String>,
    pub latest_version: Option<String>,
    pub asset_name: Option<String>,
    pub asset_download_url: Option<String>,
    pub executable_path: Option<PathBuf>,
    pub is_update_available: bool,
}

impl SongRecToolStatus {
    pub fn is_installed(&self) -> bool {
        self.executable_path.is_some()
    }
}

#[derive(Clone, Debug)]
pub struct InstalledSongRec {
    pub version: Option<String>,
    pub executable_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GitHubAsset>,
}

#[derive(Clone, Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Clone, Debug)]
pub struct RecognitionResult {
    pub title: String,
    pub subtitle: Option<String>,
    pub source: String,
    pub raw: String,
}

impl RecognitionResult {
    pub fn display_label(&self) -> String {
        match self.subtitle.as_deref().filter(|subtitle| !subtitle.trim().is_empty()) {
            Some(subtitle) => format!("{} — {}", self.title, subtitle),
            None => self.title.clone(),
        }
    }
}

#[derive(Clone, Debug)]
struct CandidateCommand {
    command: PathBuf,
    args: Vec<String>,
    source: String,
}

pub fn temporary_sample_path() -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("audio-orbit-recognition-{millis}.wav"))
}

pub fn recognize_with_songrec(command_path: Option<PathBuf>, sample_path: &Path) -> Result<RecognitionResult> {
    let candidates = songrec_candidates(command_path, sample_path);
    let mut errors = Vec::new();

    for candidate in candidates {
        let output = Command::new(&candidate.command)
            .args(&candidate.args)
            .stdin(Stdio::null())
            .output();

        let output = match output {
            Ok(output) => output,
            Err(error) => {
                errors.push(format!("{}: {error}", candidate.source));
                continue;
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();

        if !output.status.success() {
            let message = if stderr.is_empty() { stdout.clone() } else { stderr.clone() };
            errors.push(format!("{}: {}", candidate.source, if message.is_empty() { "command failed".to_owned() } else { message }));
            continue;
        }

        let raw = if stdout.is_empty() { stderr.clone() } else { stdout.clone() };
        if raw.trim().is_empty() {
            errors.push(format!("{}: empty recognition response", candidate.source));
            continue;
        }

        if let Some(result) = parse_songrec_output(&raw, &candidate.source) {
            return Ok(result);
        }

        errors.push(format!("{}: could not parse response: {}", candidate.source, raw));
    }

    if errors.iter().all(|error| error.contains("No such file") || error.contains("not found") || error.contains("os error 2")) {
        anyhow::bail!(
            "SongRec was not found. Put songrec.exe or songrec-cli.exe in .audio-orbit-dll, next to Audio Orbit, on PATH, or set the executable path in Settings > Recognition."
        );
    }

    anyhow::bail!(
        "free SongRec recognition failed. Put SongRec in .audio-orbit-dll or set its executable path in Settings > Recognition. Details: {}",
        errors.join(" | ")
    )
}

fn songrec_candidates(command_path: Option<PathBuf>, sample_path: &Path) -> Vec<CandidateCommand> {
    let mut commands = Vec::new();
    if let Some(command_path) = command_path {
        commands.push((command_path, "configured SongRec".to_owned()));
    } else {
        let tools = external_tools_dir();
        commands.push((tools.join("songrec-cli.exe"), ".audio-orbit-dll songrec-cli.exe".to_owned()));
        commands.push((tools.join("songrec.exe"), ".audio-orbit-dll songrec.exe".to_owned()));
        commands.push((tools.join("audio-file-to-recognized-song.exe"), ".audio-orbit-dll audio-file-to-recognized-song.exe".to_owned()));
        commands.push((tools.join("songrec-cli"), ".audio-orbit-dll songrec-cli".to_owned()));
        commands.push((tools.join("songrec"), ".audio-orbit-dll songrec".to_owned()));
        commands.push((tools.join("audio-file-to-recognized-song"), ".audio-orbit-dll audio-file-to-recognized-song".to_owned()));
        if let Ok(current_exe) = env::current_exe() {
            if let Some(folder) = current_exe.parent() {
                commands.push((folder.join("songrec.exe"), "app folder songrec.exe".to_owned()));
                commands.push((folder.join("songrec-cli.exe"), "app folder songrec-cli.exe".to_owned()));
                commands.push((folder.join("audio-file-to-recognized-song.exe"), "app folder audio-file-to-recognized-song.exe".to_owned()));
                commands.push((folder.join("songrec"), "app folder songrec".to_owned()));
                commands.push((folder.join("audio-file-to-recognized-song"), "app folder audio-file-to-recognized-song".to_owned()));
            }
        }
        commands.push((PathBuf::from("songrec"), "PATH songrec".to_owned()));
        commands.push((PathBuf::from("songrec.exe"), "PATH songrec.exe".to_owned()));
        commands.push((PathBuf::from("songrec-cli"), "PATH songrec-cli".to_owned()));
        commands.push((PathBuf::from("songrec-cli.exe"), "PATH songrec-cli.exe".to_owned()));
        commands.push((PathBuf::from("audio-file-to-recognized-song"), "PATH audio-file-to-recognized-song".to_owned()));
        commands.push((PathBuf::from("audio-file-to-recognized-song.exe"), "PATH audio-file-to-recognized-song.exe".to_owned()));
    }

    let sample = sample_path.display().to_string();
    let argument_sets = vec![
        vec!["recognize".to_owned(), "--json".to_owned(), sample.clone()],
        vec!["recognize".to_owned(), sample.clone(), "--json".to_owned()],
        vec!["recognize".to_owned(), sample.clone()],
        vec!["audio-file-to-recognized-song".to_owned(), sample.clone()],
        vec!["audio-file-to-recognized-song".to_owned(), sample.clone(), "--json".to_owned()],
    ];

    commands
        .into_iter()
        .flat_map(|(command, label)| {
            let argument_sets = argument_sets.clone();
            argument_sets.into_iter().map(move |args| CandidateCommand {
                command: command.clone(),
                source: format!("{label} {}", args.join(" ")),
                args,
            })
        })
        .collect()
}

pub fn cleanup_sample(path: &Path) {
    let _ = fs::remove_file(path);
}

fn parse_songrec_output(raw: &str, source: &str) -> Option<RecognitionResult> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        let title = find_json_string(&value, &["title", "track", "name"])?;
        let subtitle = find_json_string(&value, &["subtitle", "artist", "artists", "performer"]);
        return Some(RecognitionResult {
            title,
            subtitle,
            source: source.to_owned(),
            raw: trimmed.to_owned(),
        });
    }

    let line = trimmed.lines().find(|line| !line.trim().is_empty())?.trim();
    if line.eq_ignore_ascii_case("no match") || line.to_ascii_lowercase().contains("not recognized") {
        return None;
    }

    let (title, subtitle) = split_plain_song_label(line);
    if title.trim().is_empty() {
        None
    } else {
        Some(RecognitionResult {
            title,
            subtitle,
            source: source.to_owned(),
            raw: trimmed.to_owned(),
        })
    }
}

fn find_json_string(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(value) = map.get(*key) {
                    if let Some(text) = json_value_to_text(value) {
                        return Some(text);
                    }
                }
            }

            for value in map.values() {
                if let Some(text) = find_json_string(value, keys) {
                    return Some(text);
                }
            }
            None
        }
        Value::Array(values) => values.iter().find_map(|value| find_json_string(value, keys)),
        _ => None,
    }
}

fn json_value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => non_empty(text),
        Value::Array(values) => {
            let joined = values
                .iter()
                .filter_map(json_value_to_text)
                .collect::<Vec<_>>()
                .join(", ");
            non_empty(&joined)
        }
        Value::Object(map) => {
            for key in ["name", "title", "text"] {
                if let Some(text) = map.get(key).and_then(json_value_to_text) {
                    return Some(text);
                }
            }
            None
        }
        _ => None,
    }
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn split_plain_song_label(value: &str) -> (String, Option<String>) {
    for separator in [" - ", " — ", " – "] {
        if let Some((left, right)) = value.split_once(separator) {
            let left = left.trim();
            let right = right.trim();
            if !left.is_empty() && !right.is_empty() {
                return (right.to_owned(), Some(left.to_owned()));
            }
        }
    }

    (value.trim().to_owned(), None)
}

pub fn ensure_sample_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        anyhow::bail!("recognition sample was not created: {}", path.display());
    }
    let len = fs::metadata(path)
        .with_context(|| format!("failed to inspect recognition sample: {}", path.display()))?
        .len();
    if len < 256 {
        anyhow::bail!("recognition sample is too small: {}", path.display());
    }
    Ok(())
}


pub fn installed_songrec_executable() -> Option<PathBuf> {
    managed_songrec_candidates()
        .into_iter()
        .find(|path| path.is_file())
}

pub fn installed_songrec_version() -> Option<String> {
    fs::read_to_string(external_tools_dir().join(SONGREC_VERSION_FILE))
        .ok()
        .and_then(|value| non_empty(&value))
}

pub fn check_songrec_tool(include_prereleases: bool) -> Result<SongRecToolStatus> {
    let executable_path = installed_songrec_executable();
    let installed_version = installed_songrec_version();
    let client = Client::builder().user_agent(SONGREC_USER_AGENT).build()?;
    let releases: Vec<GitHubRelease> = get_github_json(&client, SONGREC_RELEASES_API, "SongRec GitHub releases")?;
    let (release, asset) = select_songrec_release_with_windows_asset(releases, include_prereleases)
        .context("no suitable SongRec GitHub release with a downloadable Windows asset was found")?;
    let latest_version = release_version_label(&release);
    let is_update_available = match (&installed_version, &latest_version) {
        (Some(installed), Some(latest)) => parse_version(latest) > parse_version(installed),
        (None, Some(_)) => executable_path.is_none(),
        _ => executable_path.is_none(),
    };

    Ok(SongRecToolStatus {
        installed_version,
        latest_version,
        asset_name: Some(asset.name),
        asset_download_url: Some(asset.browser_download_url),
        executable_path,
        is_update_available,
    })
}

pub fn install_or_update_songrec(download_url: &str, version: Option<&str>, asset_name: Option<&str>) -> Result<InstalledSongRec> {
    let tools_dir = external_tools_dir();
    fs::create_dir_all(&tools_dir)
        .with_context(|| format!("failed to create Audio Orbit tools folder: {}", tools_dir.display()))?;

    let client = Client::builder().user_agent(SONGREC_USER_AGENT).build()?;
    let bytes = client
        .get(download_url)
        .send()
        .context("failed to download SongRec release asset from GitHub")?
        .error_for_status()
        .context("SongRec GitHub release asset download failed")?
        .bytes()
        .context("failed to read SongRec GitHub release asset")?;

    let asset_name = asset_name.unwrap_or("songrec.exe");
    let lower_asset_name = asset_name.to_ascii_lowercase();
    let executable_path = if lower_asset_name.ends_with(".zip") {
        install_songrec_from_zip(&tools_dir, &bytes)?
    } else if lower_asset_name.ends_with(".exe") {
        let target = tools_dir.join(preferred_songrec_exe_name(asset_name));
        let temp = target.with_extension("download");
        fs::write(&temp, &bytes)
            .with_context(|| format!("failed to write temporary SongRec executable: {}", temp.display()))?;
        fs::rename(&temp, &target)
            .or_else(|_| {
                fs::copy(&temp, &target)?;
                let _ = fs::remove_file(&temp);
                Ok::<(), io::Error>(())
            })
            .with_context(|| format!("failed to install SongRec executable: {}", target.display()))?;
        target
    } else {
        anyhow::bail!("unsupported SongRec GitHub asset type: {asset_name}. Audio Orbit can install .zip or .exe assets.");
    };

    validate_installed_songrec(&executable_path)?;

    if let Some(version) = version.and_then(non_empty) {
        fs::write(tools_dir.join(SONGREC_VERSION_FILE), version.as_bytes())
            .with_context(|| "failed to write SongRec version marker")?;
    }

    Ok(InstalledSongRec {
        version: version.and_then(non_empty),
        executable_path,
    })
}

fn validate_installed_songrec(path: &Path) -> Result<()> {
    if !path.is_file() {
        anyhow::bail!("SongRec was downloaded but the executable was not created: {}", path.display());
    }
    let len = fs::metadata(path)
        .with_context(|| format!("failed to inspect installed SongRec executable: {}", path.display()))?
        .len();
    if len < 1024 {
        anyhow::bail!("installed SongRec executable is unexpectedly small: {}", path.display());
    }
    Ok(())
}

fn managed_songrec_candidates() -> Vec<PathBuf> {
    let tools = external_tools_dir();
    vec![
        tools.join("songrec-cli.exe"),
        tools.join("songrec.exe"),
        tools.join("audio-file-to-recognized-song.exe"),
        tools.join("songrec-cli"),
        tools.join("songrec"),
        tools.join("audio-file-to-recognized-song"),
    ]
}

fn get_github_json<T>(client: &Client, url: &str, label: &str) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let response = client
        .get(url)
        .send()
        .with_context(|| format!("failed to contact {label}"))?;

    let status = response.status();
    if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
        anyhow::bail!("GitHub temporarily refused the SongRec release check ({status}). Try again later.");
    }

    response
        .error_for_status()
        .with_context(|| format!("{label} request failed"))?
        .json()
        .with_context(|| format!("failed to parse {label} response"))
}

fn select_songrec_release_with_windows_asset(
    mut releases: Vec<GitHubRelease>,
    include_prereleases: bool,
) -> Option<(GitHubRelease, GitHubAsset)> {
    releases.retain(|release| !release.draft && (include_prereleases || !release.prerelease));
    releases.sort_by(|left, right| parse_version(&right.tag_name).cmp(&parse_version(&left.tag_name)));

    for release in releases {
        if let Some(asset) = release
            .assets
            .iter()
            .filter_map(|asset| songrec_windows_asset_rank(&asset.name).map(|rank| (rank, asset)))
            .min_by_key(|(rank, _)| *rank)
            .map(|(_, asset)| asset.clone())
        {
            return Some((release, asset));
        }
    }

    None
}

fn release_version_label(release: &GitHubRelease) -> Option<String> {
    non_empty(release.tag_name.trim_start_matches('v'))
}

fn parse_version(value: &str) -> Version {
    Version::parse(value.trim().trim_start_matches('v')).unwrap_or_else(|_| Version::new(0, 0, 0))
}

fn songrec_windows_asset_rank(name: &str) -> Option<u8> {
    let lower = name.to_ascii_lowercase();
    let looks_windows = lower.contains("windows")
        || lower.contains("win64")
        || lower.contains("win32")
        || lower.contains("x86_64-pc-windows")
        || lower.ends_with(".exe");
    let looks_downloadable = lower.ends_with(".zip") || lower.ends_with(".exe");
    let looks_cli_or_app = lower.contains("songrec") || lower.contains("audio-file-to-recognized-song");
    if !(looks_windows && looks_downloadable && looks_cli_or_app) {
        return None;
    }

    let mut rank = 100u8;
    if lower.contains("cli") || lower.contains("audio-file-to-recognized-song") {
        rank = rank.saturating_sub(30);
    }
    if lower.contains("portable") {
        rank = rank.saturating_sub(16);
    }
    if lower.contains("x86_64") || lower.contains("win64") {
        rank = rank.saturating_sub(12);
    }
    if lower.ends_with(".exe") {
        rank = rank.saturating_sub(8);
    }
    if lower.ends_with(".zip") {
        rank = rank.saturating_sub(4);
    }
    Some(rank)
}

fn preferred_songrec_exe_name(asset_name: &str) -> &'static str {
    let lower = asset_name.to_ascii_lowercase();
    if lower.contains("cli") || lower.contains("audio-file-to-recognized-song") {
        "songrec-cli.exe"
    } else {
        "songrec.exe"
    }
}

fn install_songrec_from_zip(tools_dir: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let reader = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader).context("failed to open SongRec ZIP asset")?;
    let mut selected_index = None;
    let mut selected_name = "songrec.exe".to_owned();

    for index in 0..archive.len() {
        let file = archive.by_index(index)?;
        let Some(name) = file.enclosed_name().map(|path| path.to_path_buf()) else {
            continue;
        };
        let lower = name.to_string_lossy().to_ascii_lowercase();
        if lower.ends_with(".exe") && (lower.contains("songrec") || lower.contains("audio-file-to-recognized-song")) {
            selected_index = Some(index);
            selected_name = if lower.contains("cli") || lower.contains("audio-file-to-recognized-song") {
                "songrec-cli.exe".to_owned()
            } else {
                "songrec.exe".to_owned()
            };
            break;
        }
    }

    let Some(index) = selected_index else {
        anyhow::bail!("the SongRec ZIP asset did not contain songrec.exe, songrec-cli.exe, or audio-file-to-recognized-song.exe");
    };

    let mut file = archive.by_index(index)?;
    let target = tools_dir.join(selected_name);
    let temp = target.with_extension("download");
    let mut output = fs::File::create(&temp)
        .with_context(|| format!("failed to create temporary SongRec executable: {}", temp.display()))?;
    io::copy(&mut file, &mut output)
        .with_context(|| format!("failed to extract SongRec executable: {}", target.display()))?;
    fs::rename(&temp, &target)
        .or_else(|_| {
            fs::copy(&temp, &target)?;
            let _ = fs::remove_file(&temp);
            Ok::<(), io::Error>(())
        })
        .with_context(|| format!("failed to install SongRec executable: {}", target.display()))?;
    Ok(target)
}
