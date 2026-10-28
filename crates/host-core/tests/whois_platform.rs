use std::path::{Path, PathBuf};

use host_core::whois::{
    default_localapi_path, pick_cli, windows_cli_candidates, CliSource, LocalApiSource,
};

#[test]
fn defaults_use_the_platform_endpoint() {
    #[cfg(not(windows))]
    {
        assert_eq!(default_localapi_path(), "/var/run/tailscaled.socket");
        assert_eq!(CliSource::default().executable(), Path::new("tailscale"));
    }
    #[cfg(windows)]
    {
        assert_eq!(
            default_localapi_path(),
            host_core::whois::WINDOWS_LOCALAPI_PIPE
        );
        let pf = std::env::var("ProgramFiles").ok();
        let w6432 = std::env::var("ProgramW6432").ok();
        assert_eq!(
            CliSource::default().executable(),
            pick_cli(
                &windows_cli_candidates(pf.as_deref(), w6432.as_deref()),
                |path| path.is_file()
            )
        );
    }
    assert_eq!(
        LocalApiSource::default().socket_path(),
        Path::new(default_localapi_path())
    );
}

#[test]
fn windows_candidates_prefer_native_directory_and_deduplicate() {
    assert_eq!(
        windows_cli_candidates(Some(r"C:\Program Files"), Some(r"C:\Program Files")),
        [PathBuf::from(r"C:\Program Files\Tailscale\tailscale.exe")]
    );
    assert_eq!(
        windows_cli_candidates(Some(r"C:\Program Files (x86)"), Some(r"D:\Apps\")),
        [
            PathBuf::from(r"D:\Apps\Tailscale\tailscale.exe"),
            PathBuf::from(r"C:\Program Files (x86)\Tailscale\tailscale.exe")
        ]
    );
    assert_eq!(
        windows_cli_candidates(Some(r"c:\program files"), Some(r"C:\Program Files")),
        [PathBuf::from(r"C:\Program Files\Tailscale\tailscale.exe")]
    );
    assert_eq!(
        windows_cli_candidates(Some(r"C:\Program Files/\\/"), None),
        [PathBuf::from(r"C:\Program Files\Tailscale\tailscale.exe")]
    );
}

#[test]
fn windows_candidates_use_default_for_missing_or_empty_roots() {
    for (pf, w6432) in [(None, Some("")), (None, None), (Some(""), Some(""))] {
        assert_eq!(
            windows_cli_candidates(pf, w6432),
            [PathBuf::from(r"C:\Program Files\Tailscale\tailscale.exe")]
        );
    }
}

#[test]
fn cli_selection_uses_first_existing_candidate_or_path() {
    let a = PathBuf::from(r"D:\Apps\Tailscale\tailscale.exe");
    let b = PathBuf::from(r"C:\Program Files\Tailscale\tailscale.exe");
    let candidates = [a.clone(), b.clone()];
    assert_eq!(pick_cli(&candidates, |path| path == b), b);
    assert_eq!(pick_cli(&candidates, |_| true), a);
    assert_eq!(pick_cli(&candidates, |_| false), PathBuf::from("tailscale"));
    assert_eq!(pick_cli(&[], |_| true), PathBuf::from("tailscale"));
}

#[cfg(unix)]
mod unix {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use host_core::whois::{CliSource, WhoIsError};

    struct Script {
        dir: PathBuf,
        path: PathBuf,
    }

    impl Script {
        fn new(body: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "rs-w8c-whois-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&dir).unwrap();
            let path = dir.join("tailscale");
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self { dir, path }
        }
    }

    impl Drop for Script {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.dir).unwrap();
        }
    }

    #[tokio::test]
    async fn large_cli_output_is_drained_while_waiting() {
        let script = Script::new(
            r#"printf '%s' '{"Self":{"UserID":7},"User":{"7":{"LoginName":"alice@example.com"}},"Pad":"'
head -c 300000 /dev/zero | tr '\0' x
printf '%s\n' '"}'"#,
        );
        let started = Instant::now();
        assert_eq!(
            CliSource::new(&script.path).self_login().await,
            Ok("alice@example.com".to_owned())
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn hung_cli_times_out_and_returns_promptly() {
        // Replace the shell so kill_on_drop kills the sleeper itself.
        let script = Script::new("exec sleep 30");
        let started = Instant::now();
        assert_eq!(
            CliSource::new(&script.path).self_login().await,
            Err(WhoIsError::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn unsuccessful_cli_exit_remains_a_command_error() {
        let script = Script::new("exit 7");
        let result = CliSource::new(&script.path).self_login().await;
        assert!(
            matches!(result, Err(WhoIsError::Command(message)) if message.starts_with("exit status "))
        );
    }
}
