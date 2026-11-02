//! Repo tooling, run as `cargo xtask`.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::{env, fs};

const USAGE: &str = "usage: cargo xtask <command>

commands:
  image firmware                  Build the firmware image for linux/arm64 and
                                  linux/amd64 and record their ids in
                                  docker/firmware/digest.
  firmware <nrf|esp32s3|all> [--platform linux/arm64|linux/amd64]
                                  Build kernel firmware in the pinned image, on
                                  the host's platform by default. Artifacts go
                                  to target/firmware/<arch>/.

`bench` arrives in 0.5 and `guards` in 4.12.";

const PLATFORMS: [&str; 2] = ["linux/arm64", "linux/amd64"];
const DIGEST_FILE: &str = "docker/firmware/digest";
const REGISTRY_VOLUME: &str = "mvm-firmware-cargo-registry";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        [] | ["help" | "-h" | "--help"] => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        ["image", "firmware"] => image_firmware(),
        ["firmware", family, rest @ ..] => match platform_arg(rest) {
            Ok(platform) => firmware(family, &platform),
            Err(e) => Err(e),
        },
        [command, ..] => Err(format!("unknown command `{command}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn platform_arg(rest: &[&str]) -> Result<String, String> {
    match rest {
        [] => Ok(host_platform().to_string()),
        ["--platform", p] if PLATFORMS.contains(p) => Ok(p.to_string()),
        _ => Err(format!("bad arguments {rest:?}\n\n{USAGE}")),
    }
}

fn host_platform() -> &'static str {
    match env::consts::ARCH {
        "x86_64" => "linux/amd64",
        _ => "linux/arm64",
    }
}

/// `linux/arm64` → `arm64`.
fn arch(platform: &str) -> &str {
    platform.trim_start_matches("linux/")
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let status = cmd.status().map_err(|e| format!("{cmd:?}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{cmd:?} failed: {status}"))
    }
}

fn image_firmware() -> Result<(), String> {
    let root = repo_root();
    let mut lines = String::from(
        "# Firmware build image ids, one per platform (implementation plan 0.2a).\n\
         # 0.3 replaces them with the pushed multi-arch digest.\n",
    );
    for platform in PLATFORMS {
        let tag = format!("mvm-firmware:{}", arch(platform));
        run(Command::new("docker")
            .args(["build", "--platform", platform, "-t", &tag])
            .arg(root.join("docker/firmware")))?;
        let out = Command::new("docker")
            .args(["image", "inspect", "--format", "{{.Id}}", &tag])
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("docker image inspect {tag} failed"));
        }
        let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
        lines.push_str(&format!("{platform} {id}\n"));
    }
    fs::write(root.join(DIGEST_FILE), lines).map_err(|e| e.to_string())?;
    println!("wrote {DIGEST_FILE}");
    Ok(())
}

fn image_for(platform: &str) -> Result<String, String> {
    let text = fs::read_to_string(repo_root().join(DIGEST_FILE))
        .map_err(|e| format!("{DIGEST_FILE}: {e} (run `cargo xtask image firmware`)"))?;
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| l.strip_prefix(platform)?.split_whitespace().next())
        .map(str::to_string)
        .ok_or_else(|| format!("{DIGEST_FILE} has no image for {platform}"))
}

fn firmware(family: &str, platform: &str) -> Result<(), String> {
    match family {
        "nrf" | "esp32s3" => build_family(family, platform),
        "all" => {
            build_family("nrf", platform)?;
            build_family("esp32s3", platform)
        }
        _ => Err(format!("unknown family `{family}`\n\n{USAGE}")),
    }
}

/// The commit time of `HEAD`, in seconds since the Unix epoch.
fn source_date_epoch() -> Result<String, String> {
    let out = Command::new("git")
        .args(["log", "-1", "--format=%ct"])
        .current_dir(repo_root())
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err("git log failed".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Builds one family's stub in the pinned image and converts it to the
/// flashable artifact under `target/firmware/<arch>/`.
///
/// The ESP32-S3 image is made from a stripped copy of the ELF: espflash
/// hashes the whole ELF into the app descriptor, and symbol names carry
/// crate hashes that differ between build platforms even when the code
/// is identical.
fn build_family(family: &str, platform: &str) -> Result<(), String> {
    let image = image_for(platform)?;
    let arch = arch(platform);
    let out = format!("/work/target/firmware/{arch}");
    let target = format!("{out}/build/{family}");
    let script = match family {
        "nrf" => format!(
            "cargo build --release --target-dir {target} \
             && mkdir -p {out} \
             && arm-none-eabi-objcopy -O binary {target}/thumbv7em-none-eabihf/release/mvm-nrf {out}/mvm-nrf.bin \
             && python3 /opt/uf2/uf2conv.py -c -f 0xADA52840 -b 0x26000 -o {out}/mvm-nrf.uf2 {out}/mvm-nrf.bin"
        ),
        _ => format!(
            "cargo build --release --target-dir {target} \
             && mkdir -p {out} \
             && xtensa-esp32s3-elf-strip -o {out}/mvm-esp32s3.stripped.elf {target}/xtensa-esp32s3-none-elf/release/mvm-esp32s3 \
             && espflash save-image --chip esp32s3 {out}/mvm-esp32s3.stripped.elf {out}/mvm-esp32s3.bin"
        ),
    };
    let crate_dir = match family {
        "nrf" => "/work/apps/mvm-nrf",
        _ => "/work/apps/mvm-esp32s3",
    };
    // The ESP-IDF app descriptor records a build time; pinning it to the
    // commit's time keeps the bytes the same on every machine.
    let epoch = source_date_epoch()?;
    run(Command::new("docker")
        .args(["run", "--rm", "--platform", platform])
        .args(["-e", &format!("SOURCE_DATE_EPOCH={epoch}")])
        .arg("-v")
        .arg(format!("{}:/work", repo_root().display()))
        .args([
            "-v",
            &format!("{REGISTRY_VOLUME}:/usr/local/cargo/registry"),
        ])
        .args(["-w", crate_dir, &image, "sh", "-c", &script]))
}
