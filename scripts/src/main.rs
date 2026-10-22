use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

const CARGO_TOML_PATH: &str = "Cargo.toml";
const DEFAULT_TARGET: &str = "x86_64-unknown-linux-musl";
const NFPM_CONFIG_PATH: &str = "nfpm.yaml";

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let task = args.first().map(|s| s.as_str()).unwrap_or("build");

    match task {
        "build" => build_packages(&args[1..]),
        "release:pre" => release_pre(),
        "release" => release(),
        "release:post" => release_post(),
        _ => fail(&format!("Unknown task: {}", task)),
    }
}

fn fail(message: &str) -> ! {
    eprintln!("{}", message);
    process::exit(1);
}

fn build_packages(args: &[String]) {
    let target = match args {
        [] => DEFAULT_TARGET.to_string(),
        [flag, value] if flag == "--target" => value.clone(),
        _ => fail("Usage: cargo scripts build [--target <triple>]"),
    };
    let arch =
        target_arch(&target).unwrap_or_else(|| fail(&format!("Unsupported target: {}", target)));

    let dist_dir = PathBuf::from("dist");

    // Create dist directory
    fs::create_dir_all(&dist_dir).expect("Failed to create dist directory");

    // The VERSION environment variable overrides the workspace version.
    let version = env::var("VERSION").unwrap_or_else(|_| project_version());

    println!(
        "Building packages with version: {} ({}, {})",
        version, target, arch
    );

    // nfpm doesn't expand environment variables in `contents.src`, so render
    // the target triple into a per-arch copy of the config.
    let template = fs::read_to_string(NFPM_CONFIG_PATH)
        .unwrap_or_else(|_| fail(&format!("The `{}` can't be read.", NFPM_CONFIG_PATH)));
    let config = PathBuf::from("target").join(format!("nfpm.{}.yaml", arch));
    fs::create_dir_all("target").expect("Failed to create target directory");
    fs::write(&config, template.replace("${TARGET}", &target))
        .unwrap_or_else(|_| fail(&format!("The `{}` can't be written.", config.display())));

    // Build .deb package
    run_nfpm(&config, "deb", &version, arch);

    // Build .rpm package
    run_nfpm(&config, "rpm", &version, arch);

    println!("Packages built successfully in {:?}", dist_dir);
}

fn run_nfpm(config: &Path, packager: &str, version: &str, arch: &str) {
    let output = Command::new("nfpm")
        .arg("package")
        .arg("--config")
        .arg(config)
        .arg("--packager")
        .arg(packager)
        .arg("--target")
        .arg("dist")
        .env("VERSION", version)
        .env("ARCH", arch)
        .output()
        .unwrap_or_else(|_| panic!("Failed to run nfpm for {}", packager));

    if !output.status.success() {
        eprintln!(
            "nfpm build failed for {}:\n{}",
            packager,
            String::from_utf8_lossy(&output.stderr)
        );
        process::exit(1);
    }

    println!("Built {} package", packager);
}

/// Create a release branch and PR to prepare for release.
fn release_pre() {
    let version = env::var("RELEASE_VERSION")
        .unwrap_or_else(|_| fail("You must define `RELEASE_VERSION` environment variable."));
    if !is_release_version(&version) {
        fail("The `RELEASE_VERSION` should match semver format (X.Y.Z).");
    }

    let branch = format!("feat/pre-release-v{}", version);
    bump_version_pr(
        &branch,
        &version,
        &format!("release: ignity-utils-rs v{}", version),
    );
}

/// Create a release tag and GitHub release with the built packages.
fn release() {
    let version = project_version();
    let tag = format!("v{}", version);

    if !run_status("git", &["tag", &tag]) {
        fail(&format!("The release `{}` already exists.", tag));
    }
    run("git", &["push", "origin", &tag]);

    let mut assets: Vec<String> = fs::read_dir("dist")
        .unwrap_or_else(|_| fail("The `dist` directory can't be read."))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_file() && is_package(path))
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    assets.sort();
    if assets.is_empty() {
        fail("No packages found in `dist`.");
    }

    let title = format!("ignity-utils-rs {}", tag);
    let mut args = vec![
        "release",
        "create",
        &tag,
        "--title",
        &title,
        "--generate-notes",
        "--verify-tag",
    ];
    args.extend(assets.iter().map(|s| s.as_str()));
    run("gh", &args);
}

/// Prepare for the next development iteration after release.
fn release_post() {
    let version = env::var("NEXT_VERSION")
        .unwrap_or_else(|_| fail("You must define `NEXT_VERSION` environment variable."));
    if !is_dev_version(&version) {
        fail("The `NEXT_VERSION` should match semver format (X.Y.Z-dev).");
    }

    let branch = format!("feat/post-release-v{}", version);
    bump_version_pr(&branch, &version, "chore: prepare for next iteration");
}

fn bump_version_pr(branch: &str, version: &str, message: &str) {
    if project_version() == version {
        fail(&format!("The project version is already `{}`.", version));
    }

    run("git", &["checkout", "-b", branch]);

    set_version(version);

    run("git", &["add", "--all"]);
    run("git", &["commit", "--no-verify", "-m", message]);
    run("git", &["push", "origin", branch]);
    run(
        "gh",
        &[
            "pr", "create", "--base", "main", "--head", branch, "--title", message, "--body",
            message,
        ],
    );
}

/// Update the workspace version in Cargo.toml and refresh Cargo.lock.
fn set_version(version: &str) {
    let path = Path::new(CARGO_TOML_PATH);
    let manifest = fs::read_to_string(path)
        .unwrap_or_else(|_| fail(&format!("The `{}` can't be read.", CARGO_TOML_PATH)));
    let updated = replace_workspace_version(&manifest, version).unwrap_or_else(|| {
        fail(&format!(
            "No `[workspace.package]` version found in `{}`.",
            CARGO_TOML_PATH
        ))
    });
    fs::write(path, updated)
        .unwrap_or_else(|_| fail(&format!("The `{}` can't be written.", CARGO_TOML_PATH)));

    run("cargo", &["update", "--workspace"]);
}

fn project_version() -> String {
    let manifest = fs::read_to_string(CARGO_TOML_PATH)
        .unwrap_or_else(|_| fail(&format!("The `{}` can't be read.", CARGO_TOML_PATH)));
    read_workspace_version(&manifest).unwrap_or_else(|| {
        fail(&format!(
            "No `[workspace.package]` version found in `{}`.",
            CARGO_TOML_PATH
        ))
    })
}

fn run(cmd: &str, args: &[&str]) {
    if !run_status(cmd, args) {
        fail(&format!("Command failed: {} {}", cmd, args.join(" ")));
    }
}

fn run_status(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .status()
        .unwrap_or_else(|_| fail(&format!("Failed to run {}", cmd)))
        .success()
}

fn is_package(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.starts_with("ignity-utils-rs") && (name.ends_with(".deb") || name.ends_with(".rpm"))
}

fn target_arch(target: &str) -> Option<&'static str> {
    match target {
        "x86_64-unknown-linux-musl" => Some("amd64"),
        "aarch64-unknown-linux-musl" => Some("arm64"),
        _ => None,
    }
}

fn is_release_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn is_dev_version(version: &str) -> bool {
    version.strip_suffix("-dev").is_some_and(is_release_version)
}

/// Locate the value span of `version = "..."` inside `[workspace.package]`.
fn workspace_version_span(manifest: &str) -> Option<(usize, usize)> {
    let mut in_section = false;
    let mut offset = 0;
    for line in manifest.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[workspace.package]";
        } else if in_section {
            if let Some(rest) = trimmed.strip_prefix("version") {
                if let Some(value) = rest.trim_start().strip_prefix('=') {
                    let value = value.trim();
                    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
                        let start = offset + line.find('"')? + 1;
                        return Some((start, start + value.len() - 2));
                    }
                }
            }
        }
        offset += line.len();
    }
    None
}

fn read_workspace_version(manifest: &str) -> Option<String> {
    workspace_version_span(manifest).map(|(start, end)| manifest[start..end].to_string())
}

fn replace_workspace_version(manifest: &str, version: &str) -> Option<String> {
    workspace_version_span(manifest)
        .map(|(start, end)| format!("{}{}{}", &manifest[..start], version, &manifest[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"[workspace]
members = ["crates/fix-perms", "scripts"]

[workspace.dependencies]
version = "9.9.9"

[workspace.package]
edition = "2021"
version = "0.1.0"
license = "MIT"
"#;

    #[test]
    fn reads_workspace_version() {
        assert_eq!(read_workspace_version(MANIFEST).as_deref(), Some("0.1.0"));
    }

    #[test]
    fn reads_nothing_without_workspace_package() {
        assert_eq!(
            read_workspace_version("[package]\nversion = \"1.0.0\"\n"),
            None
        );
    }

    #[test]
    fn replaces_only_workspace_version() {
        let updated = replace_workspace_version(MANIFEST, "0.2.0-dev").unwrap();
        assert_eq!(
            read_workspace_version(&updated).as_deref(),
            Some("0.2.0-dev")
        );
        assert!(updated.contains("version = \"9.9.9\""));
        assert_eq!(updated, MANIFEST.replace("\"0.1.0\"", "\"0.2.0-dev\""));
    }

    #[test]
    fn reads_root_manifest() {
        let manifest =
            fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../Cargo.toml"))
                .unwrap();
        assert!(read_workspace_version(&manifest).is_some());
    }

    #[test]
    fn validates_release_version() {
        assert!(is_release_version("0.1.0"));
        assert!(is_release_version("10.20.30"));
        assert!(!is_release_version("v0.1.0"));
        assert!(!is_release_version("0.1"));
        assert!(!is_release_version("0.1.0-dev"));
        assert!(!is_release_version("0..1"));
    }

    #[test]
    fn validates_dev_version() {
        assert!(is_dev_version("0.2.0-dev"));
        assert!(!is_dev_version("0.2.0"));
        assert!(!is_dev_version("0.2-dev"));
    }

    #[test]
    fn maps_target_to_arch() {
        assert_eq!(target_arch("x86_64-unknown-linux-musl"), Some("amd64"));
        assert_eq!(target_arch("aarch64-unknown-linux-musl"), Some("arm64"));
        assert_eq!(target_arch("x86_64-apple-darwin"), None);
    }

    #[test]
    fn detects_packages() {
        assert!(is_package(Path::new(
            "dist/ignity-utils-rs_0.1.0_amd64.deb"
        )));
        assert!(is_package(Path::new(
            "dist/ignity-utils-rs-0.1.0-1.aarch64.rpm"
        )));
        assert!(!is_package(Path::new("dist/fix-perms")));
    }
}
