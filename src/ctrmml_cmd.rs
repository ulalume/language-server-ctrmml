use std::{
    env, fs,
    io::{self, Cursor},
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use dirs::cache_dir;
use flate2::read::GzDecoder;
use reqwest::Client as HttpClient;
use tar::Archive;
use tokio::io::AsyncWriteExt;
use tokio::process::Command as TokioCommand;
use zip::ZipArchive;

const CTRMML_CMD_REPO: &str = "ulalume/ctrmml-cmd";
pub(crate) const CTRMML_CMD_NAME: &str = "ctrmml-cmd";
/// The ctrmml-cmd release downloaded when none is configured or on PATH.
pub(crate) const CTRMML_CMD_VERSION: &str = "v0.0.70";

/// Resolve the command to run, in order: the configured path,
/// `CTRMML_CMD_PATH`, `ctrmml-cmd` on PATH, then the pinned release
/// (downloaded into the cache on first use).
pub(crate) async fn resolve_command_path(
    config_path: Option<String>,
) -> std::result::Result<String, String> {
    let local = tokio::task::spawn_blocking(move || local_command_path(config_path))
        .await
        .map_err(|e| format!("failed to look up {CTRMML_CMD_NAME}: {e}"))?;
    if let Some(path) = local {
        return Ok(path);
    }
    download_ctrmml_cmd()
        .await
        .map(|path| path.to_string_lossy().into_owned())
}

fn local_command_path(config_path: Option<String>) -> Option<String> {
    if let Some(path) = config_path.as_deref().and_then(resolve_existing_command) {
        return Some(path);
    }
    if let Some(path) = env::var("CTRMML_CMD_PATH")
        .ok()
        .filter(|path| !path.is_empty())
        .and_then(|path| resolve_existing_command(&path))
    {
        return Some(path);
    }
    which_in_path(CTRMML_CMD_NAME)
}

pub(crate) async fn run_ctrmml_cmd<F>(
    cmd_path: &str,
    context: &str,
    stdin_text: Option<&str>,
    configure: F,
) -> std::result::Result<Output, String>
where
    F: FnOnce(&mut TokioCommand),
{
    let mut cmd = TokioCommand::new(cmd_path);
    configure(&mut cmd);
    if stdin_text.is_some() {
        cmd.stdin(Stdio::piped());
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to run ctrmml-cmd {context}: {e}"))?;

    if let Some(text) = stdin_text {
        write_stdin(&mut child, text).await?;
    }

    child
        .wait_with_output()
        .await
        .map_err(|e| format!("failed to run ctrmml-cmd {context}: {e}"))
}

pub(crate) fn output_message(output: &Output) -> Option<String> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let message = if !stderr.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    let message = message.trim();
    if message.is_empty() {
        None
    } else {
        Some(message.to_string())
    }
}

async fn write_stdin(
    child: &mut tokio::process::Child,
    text: &str,
) -> std::result::Result<(), String> {
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .await
            .map_err(|e| format!("failed to write ctrmml-cmd stdin: {e}"))?;
    }
    Ok(())
}

fn which_in_path(cmd: &str) -> Option<String> {
    let path_var = env::var_os("PATH")?;
    for entry in env::split_paths(&path_var) {
        let candidate = entry.join(cmd);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
        if cfg!(windows) {
            let candidate = entry.join(format!("{cmd}.exe"));
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn resolve_existing_command(path: &str) -> Option<String> {
    let candidate = Path::new(path);
    if candidate.is_file() {
        return Some(candidate.to_string_lossy().to_string());
    }
    if cfg!(windows) && !path.to_ascii_lowercase().ends_with(".exe") {
        let with_exe = candidate.with_extension("exe");
        if with_exe.is_file() {
            return Some(with_exe.to_string_lossy().to_string());
        }
    }
    None
}

fn cache_base_dir() -> PathBuf {
    cache_dir()
        .unwrap_or_else(env::temp_dir)
        .join(CTRMML_CMD_NAME)
}

fn cached_bin_name() -> String {
    if cfg!(windows) {
        format!("{CTRMML_CMD_NAME}.exe")
    } else {
        CTRMML_CMD_NAME.to_string()
    }
}

/// The most recently installed cached release, any version.
fn newest_cached_binary(base: &Path, bin_name: &str) -> Option<PathBuf> {
    fs::read_dir(base)
        .ok()?
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path().join(bin_name))
        .filter(|candidate| candidate.is_file())
        .max_by_key(|candidate| {
            fs::metadata(candidate)
                .and_then(|meta| meta.modified())
                .unwrap_or(UNIX_EPOCH)
        })
}

fn platform_asset_parts() -> std::result::Result<(&'static str, &'static str, &'static str), String>
{
    let os = match env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        "windows" => "windows",
        other => return Err(format!("unsupported platform {other}")),
    };
    let arch = match env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => return Err(format!("unsupported architecture {other}")),
    };
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    Ok((os, arch, ext))
}

/// The pinned release from the cache, downloading it first if needed. A
/// failed download falls back to the newest cached release.
async fn download_ctrmml_cmd() -> std::result::Result<PathBuf, String> {
    let base = cache_base_dir();
    let version_dir = base.join(format!("{CTRMML_CMD_NAME}-{CTRMML_CMD_VERSION}"));
    let bin_name = cached_bin_name();
    let bin_path = version_dir.join(&bin_name);
    let existing = bin_path.clone();
    if tokio::task::spawn_blocking(move || existing.is_file())
        .await
        .unwrap_or(false)
    {
        return Ok(bin_path);
    }

    let result = fetch_and_install(&base, &version_dir, &bin_name).await;
    match result {
        Ok(path) => Ok(path),
        Err(err) => {
            let (base, bin_name) = (base.clone(), bin_name.clone());
            tokio::task::spawn_blocking(move || newest_cached_binary(&base, &bin_name))
                .await
                .ok()
                .flatten()
                .ok_or(err)
        }
    }
}

async fn fetch_and_install(
    base: &Path,
    version_dir: &Path,
    bin_name: &str,
) -> std::result::Result<PathBuf, String> {
    let (os, arch, ext) = platform_asset_parts()?;
    let version = CTRMML_CMD_VERSION.trim_start_matches('v');
    let asset_name = format!("{CTRMML_CMD_NAME}-{version}-{os}-{arch}.{ext}");
    let url = format!(
        "https://github.com/{CTRMML_CMD_REPO}/releases/download/{CTRMML_CMD_VERSION}/{asset_name}"
    );
    let client = HttpClient::builder()
        .user_agent("ctrmml-lsp")
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;
    let bytes = client
        .get(&url)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|e| format!("failed to download {asset_name}: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("failed to download {asset_name}: {e}"))?;

    let (base, version_dir, bin_name) = (
        base.to_path_buf(),
        version_dir.to_path_buf(),
        bin_name.to_string(),
    );
    tokio::task::spawn_blocking(move || {
        install(&bytes, ext == "zip", &base, &version_dir, &bin_name)
            .map_err(|e| format!("failed to install {asset_name}: {e}"))
    })
    .await
    .map_err(|e| format!("failed to install {CTRMML_CMD_NAME}: {e}"))?
}

/// Extract `archive` into a private staging directory, then move it into
/// place as `version_dir` in one rename, so other runs never see a
/// partial install.
fn install(
    archive: &[u8],
    is_zip: bool,
    base: &Path,
    version_dir: &Path,
    bin_name: &str,
) -> io::Result<PathBuf> {
    fs::create_dir_all(base)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let staging = base.join(format!(".install-{}-{nonce}", std::process::id()));
    fs::create_dir(&staging)?;

    let staged = extract(archive, is_zip, &staging).and_then(|()| {
        let bin = staging.join(bin_name);
        if !bin.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{bin_name} is missing from the archive"),
            ));
        }
        make_executable(&bin)
    });
    let bin_path = version_dir.join(bin_name);
    let installed = staged.and_then(|()| {
        if version_dir.exists() && !bin_path.is_file() {
            fs::remove_dir_all(version_dir)?;
        }
        fs::rename(&staging, version_dir)
    });
    match installed {
        Ok(()) => Ok(bin_path),
        Err(err) => {
            let _ = fs::remove_dir_all(&staging);
            // Another run may have installed the same release meanwhile.
            if bin_path.is_file() {
                Ok(bin_path)
            } else {
                Err(err)
            }
        }
    }
}

fn extract(archive: &[u8], is_zip: bool, out_dir: &Path) -> io::Result<()> {
    if !is_zip {
        return Archive::new(GzDecoder::new(archive)).unpack(out_dir);
    }
    let mut zip = ZipArchive::new(Cursor::new(archive))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(relative) = entry.enclosed_name().map(Path::to_path_buf) else {
            continue;
        };
        let out_path = out_dir.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        io::copy(&mut entry, &mut fs::File::create(&out_path)?)?;
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir =
            env::temp_dir().join(format!("ctrmml-cmd-{label}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&dir).expect("create test directory");
        dir
    }

    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            writer
                .start_file(*name, zip::write::FileOptions::default())
                .expect("start zip entry");
            writer.write_all(data).expect("write zip entry");
        }
        writer.finish().expect("finish zip").into_inner()
    }

    #[test]
    fn zip_entries_outside_the_target_are_skipped() {
        let root = temp_dir("zip-slip");
        let out = root.join("out");
        fs::create_dir_all(&out).expect("create out");
        let archive = zip_bytes(&[("../escaped", b"x"), ("ctrmml-cmd", b"bin")]);

        extract(&archive, true, &out).expect("extract");

        assert!(out.join("ctrmml-cmd").is_file());
        assert!(!root.join("escaped").exists());
        fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn install_moves_a_complete_release_into_place() {
        let base = temp_dir("install");
        let version_dir = base.join("ctrmml-cmd-v0");
        let archive = zip_bytes(&[("ctrmml-cmd", b"bin"), ("LICENSE", b"text")]);

        let bin = install(&archive, true, &base, &version_dir, "ctrmml-cmd").expect("install");

        assert_eq!(bin, version_dir.join("ctrmml-cmd"));
        assert!(bin.is_file());
        let leftovers: Vec<_> = fs::read_dir(&base)
            .expect("read base")
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty());
        fs::remove_dir_all(base).expect("remove test directory");
    }

    #[test]
    fn an_archive_without_the_binary_installs_nothing() {
        let base = temp_dir("install-missing");
        let version_dir = base.join("ctrmml-cmd-v0");
        let archive = zip_bytes(&[("README", b"text")]);

        assert!(install(&archive, true, &base, &version_dir, "ctrmml-cmd").is_err());
        assert!(!version_dir.exists());
        assert_eq!(fs::read_dir(&base).expect("read base").count(), 0);
        fs::remove_dir_all(base).expect("remove test directory");
    }
}
