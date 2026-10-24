//! fix-perms — high-performance permission fixer for ignity containers.
//!
//! Reads permission spec lines from stdin.  Each line has the form:
//!
//! ```text
//! <path>  <uid>:<gid>  <fmode>  <dmode>
//! ```
//!
//! Fields are separated by whitespace (spaces or tabs).
//! Blank lines and lines starting with `#` are ignored.
//!
//! `{{USERMAP_UID}}` and `{{USERMAP_GID}}` placeholders inside the `uid:gid`
//! field are substituted with the values of the `USERMAP_UID` / `USERMAP_GID`
//! environment variables (default `0`).
//!
//! For every path that exists on the filesystem the tool:
//!   1. `chown uid:gid` every entry whose owner differs.
//!   2. `chmod fmode`   every non-directory entry whose mode differs.
//!   3. `chmod dmode`   every directory entry whose mode differs.
//!
//! The `FIX_PERMS_POLICY` environment variable mirrors Kubernetes'
//! `fsGroupChangePolicy` (case-insensitive):
//!   - `Always` (default): walk the whole tree for every spec.
//!   - `OnRootMismatch`: skip the spec entirely when the root path already has
//!     the expected owner and mode.  Entries below a matching root that have
//!     drifted are *not* fixed.
//!
//! The implementation is fully in-process (no `find`, `xargs`, `chown` or
//! `chmod` child processes), making it significantly faster than the original
//! execlineb pipeline for large trees.

use std::fs::Metadata;
use std::io::{self, BufRead};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use nix::sys::stat::Mode;
use nix::unistd::{chown, Gid, Uid};
use walkdir::WalkDir;

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

// ---------------------------------------------------------------------------
// Spec line
// ---------------------------------------------------------------------------

/// Parsed and validated permission spec for a single path.
#[derive(Debug)]
struct PermSpec {
    path: String,
    uid: u32,
    gid: u32,
    /// Mode bits for regular files (and other non-directories).
    file_mode: Mode,
    /// Mode bits for directories.
    dir_mode: Mode,
}

impl PermSpec {
    /// Parse one spec line.
    ///
    /// Returns `None` for blank / comment lines.
    fn parse(line: &str, usermap_uid: u32, usermap_gid: u32) -> Result<Option<Self>> {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return Ok(None);
        }

        let mut fields = trimmed.split_ascii_whitespace();

        let path = fields.next().ok_or("missing path field")?.to_owned();

        let raw_uidgid = fields
            .next()
            .ok_or("missing uid:gid field")?
            .replace("{{USERMAP_UID}}", &usermap_uid.to_string())
            .replace("{{USERMAP_GID}}", &usermap_gid.to_string());

        let (uid, gid) = parse_uid_gid(&raw_uidgid)?;

        let file_mode_str = fields.next().ok_or("missing fmode field")?;
        let dir_mode_str = fields.next().ok_or("missing dmode field")?;

        let file_mode = parse_mode(file_mode_str)?;
        let dir_mode = parse_mode(dir_mode_str)?;

        Ok(Some(PermSpec {
            path,
            uid,
            gid,
            file_mode,
            dir_mode,
        }))
    }
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

/// Parse `"uid:gid"` into `(u32, u32)`.
fn parse_uid_gid(s: &str) -> Result<(u32, u32)> {
    let mut parts = s.splitn(2, ':');
    let uid: u32 = parts
        .next()
        .ok_or("uid missing")?
        .parse()
        .map_err(|e| format!("invalid uid in '{s}': {e}"))?;
    let gid: u32 = parts
        .next()
        .ok_or("gid missing (expected uid:gid)")?
        .parse()
        .map_err(|e| format!("invalid gid in '{s}': {e}"))?;
    Ok((uid, gid))
}

/// Parse an octal mode string (`"0644"`, `"644"`, `"0"`, `"0000"`, etc.) into
/// `nix::sys::stat::Mode`.
fn parse_mode(s: &str) -> Result<Mode> {
    let stripped = s.trim_start_matches('0');
    // An all-zero string (e.g. "0" or "0000") is valid: it means no permissions.
    let bits: u32 = if stripped.is_empty() {
        0
    } else {
        u32::from_str_radix(stripped, 8).map_err(|e| format!("invalid mode '{s}': {e}"))?
    };
    // `mode_t` is u16 on macOS and u32 on Linux; `as _` lets the compiler pick the right width.
    // Valid permission bits (≤ 0o7777 = 4095) fit safely in either type.
    Mode::from_bits(bits as _).ok_or_else(|| format!("unknown mode bits in '{s}'").into())
}

/// Read an environment variable as a `u32`, defaulting to `0` if absent.
///
/// Unlike a silent `.ok()` chain, this emits a warning when the variable is
/// present but not a valid non-negative integer, so operators are notified
/// instead of silently having files chowned to root.
fn parse_env_u32(name: &str) -> u32 {
    match std::env::var(name) {
        Err(_) => 0, // variable not set — use default
        Ok(v) => v.parse().unwrap_or_else(|_| {
            eprintln!("fix-perms: warning: {name}={v:?} is not a valid integer; defaulting to 0");
            0
        }),
    }
}

// ---------------------------------------------------------------------------
// Change policy
// ---------------------------------------------------------------------------

/// When to traverse a spec path, mirroring Kubernetes' `fsGroupChangePolicy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangePolicy {
    /// Always walk the whole tree.
    Always,
    /// Skip the tree when its root already matches the spec.
    OnRootMismatch,
}

/// Parse a `FIX_PERMS_POLICY` value, defaulting to `Always`.
///
/// An invalid value emits a warning and falls back to `Always` — the safe
/// choice, since it never skips a fix.
fn parse_policy(value: Option<&str>) -> ChangePolicy {
    match value {
        None => ChangePolicy::Always,
        Some(v) if v.eq_ignore_ascii_case("Always") => ChangePolicy::Always,
        Some(v) if v.eq_ignore_ascii_case("OnRootMismatch") => ChangePolicy::OnRootMismatch,
        Some(v) => {
            eprintln!(
                "fix-perms: warning: FIX_PERMS_POLICY={v:?} is not one of Always, OnRootMismatch; defaulting to Always"
            );
            ChangePolicy::Always
        }
    }
}

/// Read the change policy from the `FIX_PERMS_POLICY` environment variable.
fn policy_from_env() -> ChangePolicy {
    parse_policy(std::env::var("FIX_PERMS_POLICY").ok().as_deref())
}

// ---------------------------------------------------------------------------
// Core logic
// ---------------------------------------------------------------------------

/// Compare an entry's metadata against the spec.
///
/// Returns `(owner_matches, target_perm_bits)` where `target_perm_bits` is
/// `Some(bits)` when the permission bits differ and must be changed.
fn check_entry(metadata: &Metadata, spec: &PermSpec) -> (bool, Option<u32>) {
    let owner_matches = metadata.uid() == spec.uid && metadata.gid() == spec.gid;
    let target_mode = if metadata.is_dir() {
        spec.dir_mode
    } else {
        spec.file_mode
    };
    // Compare only the permission bits (mask out file-type bits).
    let current_perm_bits = metadata.mode() & 0o7777;
    // `mode_t` is u16 on macOS and u32 on Linux; `as _` avoids an unnecessary-cast lint on Linux.
    let target_perm_bits: u32 = target_mode.bits() as _;
    let mode_change = (current_perm_bits != target_perm_bits).then_some(target_perm_bits);
    (owner_matches, mode_change)
}

/// Apply the permission spec to all entries under `spec.path`.
fn apply_spec(spec: &PermSpec, policy: ChangePolicy) -> Result<()> {
    let root = Path::new(&spec.path);
    if root.is_symlink() {
        eprintln!(
            "fix-perms: info: skipping '{}': path is a symlink",
            root.display()
        );
        return Ok(());
    }
    if !root.exists() {
        eprintln!(
            "fix-perms: info: skipping '{}': path does not exist",
            root.display()
        );
        return Ok(());
    }

    if policy == ChangePolicy::OnRootMismatch {
        // The root is known not to be a symlink, so lstat == stat here.
        let metadata = std::fs::symlink_metadata(root)?;
        if let (true, None) = check_entry(&metadata, spec) {
            eprintln!(
                "fix-perms: info: skipping '{}': root already matches (OnRootMismatch)",
                root.display()
            );
            return Ok(());
        }
    }

    let target_uid = Uid::from_raw(spec.uid);
    let target_gid = Gid::from_raw(spec.gid);
    let mut had_entry_error = false;

    for entry in WalkDir::new(root).follow_links(false) {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                eprintln!("fix-perms: warning: {err}");
                continue;
            }
        };

        // Skip symlinks entirely.
        //
        // With follow_links(false), entry.metadata() calls lstat(2), so
        // metadata.uid()/gid() reflect the symlink's own inode — not the
        // target's.  Meanwhile chown(2) and chmod(2) both follow symlinks,
        // so any syscall would mutate a different inode than the one we
        // inspected.  On Linux lchmod(2) is unavailable, so there is no safe
        // way to chmod a symlink's target that may live outside the tree.
        // Skipping symlinks matches the behaviour of the original
        // `find … -not -type l` pipeline.
        if entry.file_type().is_symlink() {
            continue;
        }

        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(err) => {
                eprintln!("fix-perms: warning: {}: {err}", entry.path().display());
                continue;
            }
        };

        let path = entry.path();
        let (owner_matches, mode_change) = check_entry(&metadata, spec);

        // ── 1. chown if owner differs ──────────────────────────────────────
        if !owner_matches {
            if let Err(err) = chown(path, Some(target_uid), Some(target_gid)) {
                eprintln!("fix-perms: warning: chown {}: {err}", path.display());
                had_entry_error = true;
            }
        }

        // ── 2. chmod according to entry type ──────────────────────────────
        // chown(2) does not modify mode bits, so cached metadata is still valid here.
        if let Some(target_perm_bits) = mode_change {
            let new_perms = std::fs::Permissions::from_mode(target_perm_bits);
            if let Err(err) = std::fs::set_permissions(path, new_perms) {
                eprintln!("fix-perms: warning: chmod {}: {err}", path.display());
                had_entry_error = true;
            }
        }
    }

    if had_entry_error {
        Err("one or more entries could not be updated (see warnings above)".into())
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    // Read template substitution values from the environment (default 0).
    //
    // If the variable is set but is not a valid non-negative integer we warn
    // and default to 0.  Silently ignoring a bad value would cause every
    // placeholder-substituted spec to chown files to root without any signal
    // to the operator.
    let usermap_uid: u32 = parse_env_u32("USERMAP_UID");
    let usermap_gid: u32 = parse_env_u32("USERMAP_GID");
    let policy = policy_from_env();

    let stdin = io::stdin();
    let mut had_error = false;

    for (lineno, line) in stdin.lock().lines().enumerate() {
        let line = match line {
            Ok(l) => l,
            Err(err) => {
                eprintln!(
                    "fix-perms: error reading stdin at line {}: {err}",
                    lineno + 1
                );
                had_error = true;
                break;
            }
        };

        let spec = match PermSpec::parse(&line, usermap_uid, usermap_gid) {
            Ok(Some(s)) => s,
            Ok(None) => continue, // blank / comment
            Err(err) => {
                eprintln!("fix-perms: error on line {}: {err}", lineno + 1);
                had_error = true;
                continue;
            }
        };

        if let Err(err) = apply_spec(&spec, policy) {
            eprintln!("fix-perms: error applying spec for '{}': {err}", spec.path);
            had_error = true;
        }
    }

    if had_error {
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── parse helpers ───────────────────────────────────────────────────────

    #[test]
    fn test_parse_uid_gid_static() {
        let (uid, gid) = parse_uid_gid("0:0").unwrap();
        assert_eq!(uid, 0);
        assert_eq!(gid, 0);
    }

    #[test]
    fn test_parse_uid_gid_non_root() {
        let (uid, gid) = parse_uid_gid("1000:1001").unwrap();
        assert_eq!(uid, 1000);
        assert_eq!(gid, 1001);
    }

    #[test]
    fn test_parse_uid_gid_missing_gid() {
        assert!(parse_uid_gid("1000").is_err());
    }

    #[test]
    fn test_parse_mode_with_leading_zero() {
        let m = parse_mode("0644").unwrap();
        assert_eq!(m.bits(), 0o644);
    }

    #[test]
    fn test_parse_mode_without_leading_zero() {
        let m = parse_mode("755").unwrap();
        assert_eq!(m.bits(), 0o755);
    }

    #[test]
    fn test_parse_mode_invalid() {
        assert!(parse_mode("999").is_err());
    }

    #[test]
    fn test_parse_mode_zero() {
        // "0" and "0000" must parse to mode 0 (no permissions), not error.
        let m = parse_mode("0").unwrap();
        assert_eq!(m.bits(), 0);
        let m = parse_mode("0000").unwrap();
        assert_eq!(m.bits(), 0);
    }

    // ── PermSpec::parse ─────────────────────────────────────────────────────

    #[test]
    fn test_spec_blank_line_returns_none() {
        assert!(PermSpec::parse("", 0, 0).unwrap().is_none());
        assert!(PermSpec::parse("   ", 0, 0).unwrap().is_none());
    }

    #[test]
    fn test_spec_comment_returns_none() {
        assert!(PermSpec::parse("# this is a comment", 0, 0)
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_spec_static_uid_gid() {
        let spec = PermSpec::parse("/var/www 0:0 0600 0700", 1000, 1000)
            .unwrap()
            .unwrap();
        assert_eq!(spec.path, "/var/www");
        assert_eq!(spec.uid, 0);
        assert_eq!(spec.gid, 0);
        assert_eq!(spec.file_mode.bits(), 0o600);
        assert_eq!(spec.dir_mode.bits(), 0o700);
    }

    #[test]
    fn test_spec_dynamic_uid_gid_substitution() {
        let spec = PermSpec::parse(
            "/data {{USERMAP_UID}}:{{USERMAP_GID}} 0644 0755",
            1000,
            2000,
        )
        .unwrap()
        .unwrap();
        assert_eq!(spec.uid, 1000);
        assert_eq!(spec.gid, 2000);
    }

    #[test]
    fn test_spec_tab_separated() {
        let spec = PermSpec::parse("/tmp\t0:0\t0600\t0700", 0, 0)
            .unwrap()
            .unwrap();
        assert_eq!(spec.path, "/tmp");
        assert_eq!(spec.file_mode.bits(), 0o600);
    }

    #[test]
    fn test_spec_missing_dmode() {
        assert!(PermSpec::parse("/tmp 0:0 0600", 0, 0).is_err());
    }

    // ── apply_spec ──────────────────────────────────────────────────────────

    #[test]
    fn test_apply_spec_skips_symlinked_root() {
        let base = std::env::temp_dir().join(format!("fix-perms-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let real = base.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let file = real.join("file");
        std::fs::write(&file, b"x").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).unwrap();

        let link = base.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let meta = std::fs::metadata(&file).unwrap();
        let spec = PermSpec {
            path: link.to_string_lossy().into_owned(),
            uid: meta.uid(),
            gid: meta.gid(),
            file_mode: Mode::from_bits(0o644).unwrap(),
            dir_mode: Mode::from_bits(0o755).unwrap(),
        };
        apply_spec(&spec, ChangePolicy::Always).unwrap();

        let file_mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        let dir_mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        let _ = std::fs::remove_dir_all(&base);

        assert_eq!(file_mode, 0o600, "target file must not be touched");
        assert_eq!(dir_mode, 0o700, "target dir must not be touched");
    }

    // ── change policy ───────────────────────────────────────────────────────

    #[test]
    fn test_parse_policy() {
        assert_eq!(parse_policy(None), ChangePolicy::Always);
        assert_eq!(parse_policy(Some("Always")), ChangePolicy::Always);
        assert_eq!(
            parse_policy(Some("OnRootMismatch")),
            ChangePolicy::OnRootMismatch
        );
        assert_eq!(
            parse_policy(Some("onrootmismatch")),
            ChangePolicy::OnRootMismatch
        );
        assert_eq!(parse_policy(Some("bogus")), ChangePolicy::Always);
    }

    /// Create `<tmp>/<name>/root/child` with the given modes and return
    /// `(base, root, child)`.
    fn make_tree(name: &str, root_mode: u32, child_mode: u32) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("fix-perms-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let child = root.join("child");
        std::fs::write(&child, b"x").unwrap();
        std::fs::set_permissions(&child, std::fs::Permissions::from_mode(child_mode)).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(root_mode)).unwrap();
        (base, root, child)
    }

    fn spec_for(path: &Path) -> PermSpec {
        let meta = std::fs::metadata(path).unwrap();
        PermSpec {
            path: path.to_string_lossy().into_owned(),
            uid: meta.uid(),
            gid: meta.gid(),
            file_mode: Mode::from_bits(0o644).unwrap(),
            dir_mode: Mode::from_bits(0o755).unwrap(),
        }
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn test_on_root_mismatch_skips_matching_root() {
        let (base, root, child) = make_tree("orm-skip", 0o755, 0o600);
        apply_spec(&spec_for(&root), ChangePolicy::OnRootMismatch).unwrap();
        let child_mode = mode_of(&child);
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(child_mode, 0o600, "child must not be touched");
    }

    #[test]
    fn test_always_fixes_below_matching_root() {
        let (base, root, child) = make_tree("always", 0o755, 0o600);
        apply_spec(&spec_for(&root), ChangePolicy::Always).unwrap();
        let child_mode = mode_of(&child);
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(child_mode, 0o644);
    }

    #[test]
    fn test_on_root_mismatch_walks_mismatching_root() {
        let (base, root, child) = make_tree("orm-walk", 0o700, 0o600);
        apply_spec(&spec_for(&root), ChangePolicy::OnRootMismatch).unwrap();
        let (root_mode, child_mode) = (mode_of(&root), mode_of(&child));
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(root_mode, 0o755);
        assert_eq!(child_mode, 0o644);
    }

    #[test]
    fn test_on_root_mismatch_matching_file_root() {
        let (base, _root, child) = make_tree("orm-file", 0o755, 0o644);
        apply_spec(&spec_for(&child), ChangePolicy::OnRootMismatch).unwrap();
        let child_mode = mode_of(&child);
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(child_mode, 0o644);
    }
}
