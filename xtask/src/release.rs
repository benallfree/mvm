//! `cargo xtask release <unit> <level>` (implementation plan 0.3a).
//!
//! Bumps a release unit's version, moves its changelog's "Unreleased"
//! section under the new version, commits, tags `<unit>-vX.Y.Z`, and pushes
//! to `main`. The release workflow then publishes the GitHub Release.

use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    BUILDER_REPO, DIGEST_FILE, output, push_firmware_builder, pushed_digest, repo_root, run,
};

struct Unit {
    name: &'static str,
    changelog: &'static str,
    /// Files whose first `version = …` line holds the unit's version, kept equal.
    version_files: &'static [&'static str],
    /// `(Cargo.lock, package)` entries that record that version.
    locks: &'static [(&'static str, &'static str)],
}

const UNITS: [Unit; 4] = [
    Unit {
        name: "mesh",
        changelog: "changelog/mesh.md",
        version_files: &["apps/mesh/Cargo.toml"],
        locks: &[("Cargo.lock", "mesh")],
    },
    Unit {
        name: "mvm",
        changelog: "changelog/mvm.md",
        version_files: &["apps/mvm-nrf/Cargo.toml", "apps/mvm-esp32s3/Cargo.toml"],
        locks: &[
            ("apps/mvm-nrf/Cargo.lock", "mvm-nrf"),
            ("apps/mvm-esp32s3/Cargo.lock", "mvm-esp32s3"),
        ],
    },
    Unit {
        name: "sdk",
        changelog: "changelog/sdk.md",
        version_files: &["crates/mvm-sdk/Cargo.toml", "crates/mvm-packer/Cargo.toml"],
        locks: &[("Cargo.lock", "mvm-sdk"), ("Cargo.lock", "mvm-packer")],
    },
    Unit {
        name: "firmware-builder-image",
        changelog: "changelog/firmware-builder-image.md",
        version_files: &["docker/firmware-builder/versions"],
        locks: &[],
    },
];

pub fn release(unit: &str, level: &str) -> Result<(), String> {
    let unit = UNITS
        .iter()
        .find(|u| u.name == unit)
        .ok_or_else(|| format!("unknown release unit `{unit}`"))?;
    let root = repo_root();
    let read = |path: &str| fs::read_to_string(root.join(path)).map_err(|e| format!("{path}: {e}"));
    let write = |path: &str, text: &str| {
        fs::write(root.join(path), text).map_err(|e| format!("{path}: {e}"))
    };

    check_main_is_clean()?;

    let old = version_of(&read(unit.version_files[0])?)
        .ok_or_else(|| format!("no version in {}", unit.version_files[0]))?;
    let new = bump(&old, level)?;
    let tag = format!("{}-v{new}", unit.name);
    println!("releasing {} {old} -> {new}", unit.name);

    for path in unit.version_files {
        write(path, &set_version(&read(path)?, &old, &new)?)?;
    }
    for (lock, package) in unit.locks {
        write(lock, &set_lock_version(&read(lock)?, package, &old, &new)?)?;
    }
    write(
        unit.changelog,
        &move_unreleased(&read(unit.changelog)?, &new, &today())?,
    )?;

    if unit.name == "firmware-builder-image" {
        let image = format!("{BUILDER_REPO}:{new}");
        match pushed_digest(&image)? {
            None => push_firmware_builder(&new)?,
            Some(digest) => write(
                DIGEST_FILE,
                &format!(
                    "# Firmware builder image (implementation plan 0.2a, 0.3a): the multi-arch\n\
                     # image on Docker Hub, used by digest.\n\
                     image {image}@{digest}\n"
                ),
            )?,
        }
    }

    let git = |args: &[&str]| run(Command::new("git").args(args).current_dir(&root));
    git(&["add", "-A"])?;
    git(&["commit", "-m", &format!("Release {} {new}", unit.name)])?;
    git(&["tag", "-a", &tag, "-m", &format!("{} {new}", unit.name)])?;
    git(&["push", "origin", "main", &tag])?;
    println!("pushed {tag}; the release workflow publishes the GitHub Release");
    Ok(())
}

/// A release starts from a clean tree on `main`, level with `origin/main`.
fn check_main_is_clean() -> Result<(), String> {
    let root = repo_root();
    let git = |args: &[&str]| output(Command::new("git").args(args).current_dir(&root));
    if !git(&["status", "--porcelain"])?.is_empty() {
        return Err("the working tree has uncommitted changes".into());
    }
    if git(&["branch", "--show-current"])? != "main" {
        return Err("releases are made from main".into());
    }
    git(&["fetch", "-q", "origin", "main"])?;
    if git(&["rev-parse", "HEAD"])? != git(&["rev-parse", "origin/main"])? {
        return Err("main is not level with origin/main".into());
    }
    Ok(())
}

/// The value of the first `version = …` line, with any quotes removed.
fn version_of(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix("version = "))
        .map(|v| v.trim().trim_matches('"').to_string())
}

fn bump(version: &str, level: &str) -> Result<String, String> {
    let parts: Vec<u64> = version
        .split('.')
        .map(|p| p.parse().map_err(|_| format!("bad version `{version}`")))
        .collect::<Result<_, _>>()?;
    let [major, minor, patch] = parts[..] else {
        return Err(format!("bad version `{version}`"));
    };
    Ok(match level {
        "major" => format!("{}.0.0", major + 1),
        "minor" => format!("{major}.{}.0", minor + 1),
        "patch" => format!("{major}.{minor}.{}", patch + 1),
        _ => {
            return Err(format!(
                "unknown level `{level}`; use major, minor, or patch"
            ));
        }
    })
}

/// Replaces the first `version = …` line, keeping its quoting.
fn set_version(text: &str, old: &str, new: &str) -> Result<String, String> {
    for (from, to) in [
        (
            format!("version = \"{old}\""),
            format!("version = \"{new}\""),
        ),
        (format!("version = {old}"), format!("version = {new}")),
    ] {
        if text.contains(&from) {
            return Ok(text.replacen(&from, &to, 1));
        }
    }
    Err(format!("version {old} not found"))
}

fn set_lock_version(text: &str, package: &str, old: &str, new: &str) -> Result<String, String> {
    let from = format!("name = \"{package}\"\nversion = \"{old}\"");
    if !text.contains(&from) {
        return Err(format!("{package} {old} not found in its Cargo.lock"));
    }
    Ok(text.replacen(
        &from,
        &format!("name = \"{package}\"\nversion = \"{new}\""),
        1,
    ))
}

/// Moves the "Unreleased" entries under a new version heading, leaving an
/// empty "Unreleased" section above it.
fn move_unreleased(text: &str, version: &str, date: &str) -> Result<String, String> {
    let heading = "## [Unreleased]\n";
    if !text.contains(heading) {
        return Err("the changelog has no `## [Unreleased]` heading".into());
    }
    Ok(text.replacen(heading, &format!("{heading}\n## [{version}] - {date}\n"), 1))
}

/// Today's date in UTC as `YYYY-MM-DD` (Howard Hinnant's `civil_from_days`).
fn today() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    civil_date(secs as i64 / 86_400)
}

fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bumps_each_level() {
        assert_eq!(bump("0.0.0", "patch").unwrap(), "0.0.1");
        assert_eq!(bump("0.1.3", "minor").unwrap(), "0.2.0");
        assert_eq!(bump("1.4.2", "major").unwrap(), "2.0.0");
        assert!(bump("0.1", "patch").is_err());
        assert!(bump("0.1.0", "huge").is_err());
    }

    #[test]
    fn sets_quoted_and_bare_versions() {
        let toml = "[package]\nname = \"mesh\"\nversion = \"0.0.0\"\n";
        assert_eq!(version_of(toml).unwrap(), "0.0.0");
        assert!(
            set_version(toml, "0.0.0", "0.0.1")
                .unwrap()
                .contains("version = \"0.0.1\"")
        );
        let versions = "# pins\nversion = 0.0.0\nrust = 1.99.1\n";
        assert_eq!(version_of(versions).unwrap(), "0.0.0");
        assert!(
            set_version(versions, "0.0.0", "0.1.0")
                .unwrap()
                .contains("version = 0.1.0\nrust")
        );
    }

    #[test]
    fn sets_the_lock_entry() {
        let lock = "[[package]]\nname = \"mesh\"\nversion = \"0.0.0\"\n";
        assert_eq!(
            set_lock_version(lock, "mesh", "0.0.0", "0.0.1").unwrap(),
            "[[package]]\nname = \"mesh\"\nversion = \"0.0.1\"\n"
        );
    }

    #[test]
    fn moves_unreleased_entries() {
        let log = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- A thing.\n";
        assert_eq!(
            move_unreleased(log, "0.0.1", "2026-11-02").unwrap(),
            "# Changelog\n\n## [Unreleased]\n\n## [0.0.1] - 2026-11-02\n\n### Added\n\n- A thing.\n"
        );
    }

    #[test]
    fn formats_civil_dates() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(20_759), "2026-11-02");
        assert_eq!(civil_date(11_016), "2000-02-29");
    }
}
