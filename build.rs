use std::{fs, process::Command};

fn main() {
    configure_version_label();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("assets/audio-orbit.ico");
    resource.set_manifest(include_str!("src/audio-orbit.exe.manifest"));

    if let Err(error) = resource.compile() {
        panic!("failed to compile Windows resources: {error}");
    }
}

fn configure_version_label() {
    println!("cargo:rerun-if-env-changed=AUDIO_ORBIT_DEV_VERSION");
    println!("cargo:rerun-if-env-changed=AUDIO_ORBIT_RELEASE_BUILD");
    register_git_rerun_paths();

    let package_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_owned());
    let profile = std::env::var("PROFILE").unwrap_or_default();
    let release_build = std::env::var_os("AUDIO_ORBIT_RELEASE_BUILD").is_some() || profile == "release";

    let display_version = if release_build {
        format!("v{package_version}")
    } else if let Ok(dev_version) = std::env::var("AUDIO_ORBIT_DEV_VERSION") {
        normalize_dev_version(&dev_version)
    } else {
        let commit = git_short_commit().unwrap_or_else(|| "dev".to_owned());
        format!("v0.0.0-{commit}")
    };

    println!("cargo:rustc-env=AUDIO_ORBIT_DISPLAY_VERSION={display_version}");
}

fn normalize_dev_version(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "v0.0.0-dev".to_owned()
    } else if trimmed.starts_with('v') {
        trimmed.to_owned()
    } else {
        format!("v{trimmed}")
    }
}

fn git_short_commit() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let commit = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!commit.is_empty()).then_some(commit)
}

fn register_git_rerun_paths() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/packed-refs");

    let Ok(head) = fs::read_to_string(".git/HEAD") else {
        return;
    };

    let Some(ref_name) = head.trim().strip_prefix("ref: ") else {
        return;
    };

    println!("cargo:rerun-if-changed=.git/{ref_name}");
}
