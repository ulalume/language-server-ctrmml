use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::{Duration, SystemTime},
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
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);
const UPDATE_CHECK_FILENAME: &str = "last_update_check";

#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(serde::Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

pub(crate) async fn resolve_command_path(
    config_path: Option<String>,
) -> std::result::Result<String, String> {
    if let Some(path) = config_path {
        if let Some(existing) = resolve_existing_command(&path) {
            return Ok(existing);
        }
    }

    if let Ok(path) = env::var("CTRMML_CMD_PATH") {
        if !path.is_empty() {
            if let Some(existing) = resolve_existing_command(&path) {
                return Ok(existing);
            }
        }
    }

    if let Some(path) = which_in_path(CTRMML_CMD_NAME) {
        return Ok(path);
    }

    match download_ctrmml_cmd().await {
        Ok(Some(path)) => Ok(path.to_string_lossy().to_string()),
        Ok(None) => Ok(CTRMML_CMD_NAME.to_string()),
        Err(err) => Err(err),
    }
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

fn cache_base_dir() -> PathBuf {
    cache_dir().unwrap_or_else(env::temp_dir).join("ctrmml-cmd")
}

fn update_check_path() -> PathBuf {
    cache_base_dir().join(UPDATE_CHECK_FILENAME)
}

fn read_last_update_check() -> Option<SystemTime> {
    let contents = fs::read_to_string(update_check_path()).ok()?;
    let secs = contents.trim().parse::<u64>().ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

fn update_check_due() -> bool {
    match read_last_update_check() {
        Some(last) => last
            .elapsed()
            .map(|elapsed| elapsed >= UPDATE_CHECK_INTERVAL)
            .unwrap_or(true),
        None => true,
    }
}

fn record_update_check() {
    let secs = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => return,
    };
    let path = update_check_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, secs.to_string());
}

fn cached_bin_name() -> String {
    if env::consts::OS == "windows" {
        format!("{CTRMML_CMD_NAME}.exe")
    } else {
        CTRMML_CMD_NAME.to_string()
    }
}

fn find_cached_binary() -> Option<PathBuf> {
    let base = cache_base_dir();
    let mut best: Option<(SystemTime, PathBuf)> = None;
    let bin_name = cached_bin_name();

    let entries = fs::read_dir(base).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let candidate = path.join(&bin_name);
        if candidate.is_file() {
            let modified = fs::metadata(&candidate)
                .and_then(|meta| meta.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let replace = match &best {
                Some((current, _)) => modified > *current,
                None => true,
            };
            if replace {
                best = Some((modified, candidate));
            }
        } else if env::consts::OS == "windows" {
            let alt = path.join(CTRMML_CMD_NAME);
            if alt.is_file() {
                let modified = fs::metadata(&alt)
                    .and_then(|meta| meta.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                let replace = match &best {
                    Some((current, _)) => modified > *current,
                    None => true,
                };
                if replace {
                    best = Some((modified, alt));
                }
            }
        }
    }

    best.map(|(_, path)| path)
}

async fn fetch_latest_release(client: &HttpClient) -> std::result::Result<GithubRelease, String> {
    let url = format!("https://api.github.com/repos/{CTRMML_CMD_REPO}/releases/latest");
    let release = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("failed to fetch release: {e}"))?
        .error_for_status()
        .map_err(|e| format!("failed to fetch release: {e}"))?
        .json::<GithubRelease>()
        .await
        .map_err(|e| format!("failed to parse release: {e}"))?;
    Ok(release)
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

fn platform_asset_parts() -> std::result::Result<(String, String, String), String> {
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
    Ok((os.to_string(), arch.to_string(), ext.to_string()))
}

fn extract_zip(archive_path: &Path, out_dir: &Path) -> std::result::Result<(), String> {
    let file = fs::File::open(archive_path).map_err(|e| format!("failed to open zip: {e}"))?;
    let mut zip = ZipArchive::new(file).map_err(|e| format!("invalid zip: {e}"))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("zip read failed: {e}"))?;
        let out_path = out_dir.join(entry.name());
        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| format!("failed to create dir: {e}"))?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("failed to create dir: {e}"))?;
            }
            let mut out =
                fs::File::create(&out_path).map_err(|e| format!("failed to create file: {e}"))?;
            io::copy(&mut entry, &mut out).map_err(|e| format!("failed to extract file: {e}"))?;
        }
    }
    Ok(())
}

fn extract_targz(archive_path: &Path, out_dir: &Path) -> std::result::Result<(), String> {
    let file = fs::File::open(archive_path).map_err(|e| format!("failed to open tar.gz: {e}"))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    archive
        .unpack(out_dir)
        .map_err(|e| format!("failed to unpack tar.gz: {e}"))?;
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::result::Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .map_err(|e| format!("failed to read permissions: {e}"))?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).map_err(|e| format!("failed to set permissions: {e}"))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> std::result::Result<(), String> {
    Ok(())
}

async fn download_ctrmml_cmd() -> std::result::Result<Option<PathBuf>, String> {
    let cached_path = find_cached_binary();
    if cached_path.is_some() && !update_check_due() {
        return Ok(cached_path);
    }

    let (os, arch, ext) = platform_asset_parts()?;
    let client = HttpClient::builder()
        .user_agent("ctrmml-lsp")
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;

    let release = match fetch_latest_release(&client).await {
        Ok(release) => {
            record_update_check();
            release
        }
        Err(err) => {
            if let Some(path) = cached_path.as_ref() {
                record_update_check();
                return Ok(Some(path.clone()));
            }
            return Err(err);
        }
    };

    let version = release.tag_name.trim_start_matches('v');
    let asset_name = format!(
        "{name}-{version}-{os}-{arch}.{ext}",
        name = CTRMML_CMD_NAME,
        version = version,
        os = os,
        arch = arch,
        ext = ext,
    );
    let asset = match release.assets.iter().find(|asset| asset.name == asset_name) {
        Some(asset) => asset,
        None => {
            if let Some(path) = cached_path.as_ref() {
                return Ok(Some(path.clone()));
            }
            return Err(format!("no asset found matching {asset_name}"));
        }
    };

    let base = cache_base_dir();
    let version_dir = base.join(format!("{CTRMML_CMD_NAME}-{}", release.tag_name));
    let bin_name = if os == "windows" {
        format!("{CTRMML_CMD_NAME}.exe")
    } else {
        CTRMML_CMD_NAME.to_string()
    };
    let bin_path = version_dir.join(&bin_name);
    if bin_path.is_file() {
        return Ok(Some(bin_path));
    }

    fs::create_dir_all(&version_dir).map_err(|e| format!("failed to create cache dir: {e}"))?;

    let tmp_path = version_dir.join(format!("download.{ext}"));
    let bytes = match client
        .get(&asset.browser_download_url)
        .send()
        .await
        .map_err(|e| format!("failed to download asset: {e}"))?
        .error_for_status()
        .map_err(|e| format!("failed to download asset: {e}"))?
        .bytes()
        .await
    {
        Ok(bytes) => bytes,
        Err(e) => {
            if let Some(path) = cached_path.as_ref() {
                return Ok(Some(path.clone()));
            }
            return Err(format!("failed to read asset: {e}"));
        }
    };
    fs::write(&tmp_path, &bytes).map_err(|e| format!("failed to write asset: {e}"))?;

    if ext == "zip" {
        if let Err(err) = extract_zip(&tmp_path, &version_dir) {
            if let Some(path) = cached_path.as_ref() {
                return Ok(Some(path.clone()));
            }
            return Err(err);
        }
    } else {
        if let Err(err) = extract_targz(&tmp_path, &version_dir) {
            if let Some(path) = cached_path.as_ref() {
                return Ok(Some(path.clone()));
            }
            return Err(err);
        }
    }
    let _ = fs::remove_file(&tmp_path);

    if !bin_path.is_file() {
        if let Some(path) = cached_path.as_ref() {
            return Ok(Some(path.clone()));
        }
        return Err(format!(
            "ctrmml-cmd binary not found after extracting {asset_name}"
        ));
    }
    make_executable(&bin_path)?;
    Ok(Some(bin_path))
}
