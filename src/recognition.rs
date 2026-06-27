use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

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
    args: Vec<String>,
    source: &'static str,
}

pub fn temporary_sample_path() -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("audio-orbit-recognition-{millis}.wav"))
}

pub fn recognize_with_songrec(command_path: Option<PathBuf>, sample_path: &Path) -> Result<RecognitionResult> {
    let command_path = command_path.unwrap_or_else(|| PathBuf::from("songrec"));
    let sample = sample_path.display().to_string();
    let candidates = vec![
        CandidateCommand {
            args: vec!["recognize".to_owned(), "--json".to_owned(), sample.clone()],
            source: "SongRec recognize --json",
        },
        CandidateCommand {
            args: vec!["recognize".to_owned(), sample.clone(), "--json".to_owned()],
            source: "SongRec recognize --json",
        },
        CandidateCommand {
            args: vec!["audio-file-to-recognized-song".to_owned(), sample],
            source: "SongRec legacy audio-file-to-recognized-song",
        },
    ];

    let mut errors = Vec::new();
    for candidate in candidates {
        let output = Command::new(&command_path)
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

        if let Some(result) = parse_songrec_output(&raw, candidate.source) {
            return Ok(result);
        }

        errors.push(format!("{}: could not parse response: {}", candidate.source, raw));
    }

    anyhow::bail!(
        "free SongRec recognition failed. Install SongRec or set its executable path in Settings → Recognition. Details: {}",
        errors.join(" | ")
    )
}

pub fn cleanup_sample(path: &Path) {
    let _ = fs::remove_file(path);
}

fn parse_songrec_output(raw: &str, source: &'static str) -> Option<RecognitionResult> {
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
