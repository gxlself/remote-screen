//! 安装包下载。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;

use super::prepare::Downloader;

/// [`Downloader`] backed by the system `curl`.
pub struct CurlDownloader {
    pub program: PathBuf,
    pub connect_timeout: Duration,
    pub stall_timeout: Duration,
}

impl Default for CurlDownloader {
    fn default() -> Self {
        Self {
            program: PathBuf::from("curl"),
            connect_timeout: Duration::from_secs(10),
            stall_timeout: Duration::from_secs(60),
        }
    }
}

#[async_trait]
impl Downloader for CurlDownloader {
    async fn download(&self, url: &str, dest: &Path, max_bytes: u64) -> Result<(), String> {
        if !has_http_scheme(url) {
            return Err("unsupported url scheme".to_owned());
        }

        tracing::debug!(
            event = "upgrade.download",
            host = url_host(url),
            "starting package download"
        );

        let mut command = Command::new(&self.program);
        command
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .arg("--fail")
            .arg("--silent")
            .arg("--show-error")
            .arg("--location")
            .arg("--max-redirs")
            .arg("5")
            .arg("--proto")
            .arg("=http,https")
            .arg("--proto-redir")
            .arg("=http,https")
            .arg("--max-filesize")
            .arg(max_bytes.to_string())
            .arg("--connect-timeout")
            .arg(timeout_secs(self.connect_timeout))
            .arg("--speed-limit")
            .arg("1024")
            .arg("--speed-time")
            .arg(timeout_secs(self.stall_timeout))
            .arg("--output")
            .arg(dest)
            .arg("--url")
            .arg(url);

        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err("curl not found".to_owned());
            }
            Err(error) => return Err(format!("curl spawn failed: {error}")),
        };
        let output = match child.wait_with_output().await {
            Ok(output) => output,
            Err(error) => {
                remove_if_exists(dest).await;
                return Err(format!("curl wait failed: {error}"));
            }
        };

        let Some(code) = output.status.code() else {
            remove_if_exists(dest).await;
            return Err("curl killed".to_owned());
        };
        if code != 0 {
            remove_if_exists(dest).await;
            let stderr = String::from_utf8_lossy(&output.stderr);
            let first_line: String = stderr
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            return Err(format!("curl exit {code}: {first_line}"));
        }

        let size = match tokio::fs::metadata(dest).await {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                remove_if_exists(dest).await;
                return Err(format!("could not inspect downloaded package: {error}"));
            }
        };
        if size > max_bytes {
            remove_if_exists(dest).await;
            return Err("package too large".to_owned());
        }

        tracing::debug!(
            event = "upgrade.download",
            host = url_host(url),
            size,
            "package downloaded"
        );
        Ok(())
    }
}

fn has_http_scheme(url: &str) -> bool {
    url.get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || url
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
}

fn timeout_secs(timeout: Duration) -> String {
    let seconds = timeout.as_secs().max(1);
    let seconds = if timeout.subsec_nanos() > 0 {
        seconds.saturating_add(1)
    } else {
        seconds
    };
    seconds.to_string()
}

fn url_host(url: &str) -> &str {
    let authority = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    authority.rsplit('@').next().unwrap_or_default()
}

async fn remove_if_exists(path: &Path) {
    match tokio::fs::remove_file(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(
                event = "upgrade.download",
                result = "cleanup_error",
                "could not remove a failed download: {error}"
            );
        }
    }
}
