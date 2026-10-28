use std::path::PathBuf;

use host_core::user_dirs::{DirsOwner, EnvDirs, UserDirs, UserDirsSource};

fn with_home(value: Option<&str>, f: impl FnOnce()) {
    let previous = std::env::var_os("HOME");
    match value {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    f();
    match previous {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
}

fn expected(home: Option<&str>) -> UserDirs {
    UserDirs {
        downloads: PathBuf::from("/d"),
        home: home.map(PathBuf::from),
        owner: DirsOwner::User,
    }
}

#[test]
fn env_dirs_reads_home() {
    with_home(Some("/x"), || {
        assert_eq!(
            EnvDirs::new(PathBuf::from("/d")).current(),
            expected(Some("/x"))
        );
    });
}

#[test]
fn empty_home_is_missing() {
    with_home(Some(""), || {
        assert_eq!(EnvDirs::new(PathBuf::from("/d")).current(), expected(None));
    });
}

#[test]
fn unset_home_is_missing() {
    with_home(None, || {
        assert_eq!(EnvDirs::new(PathBuf::from("/d")).current(), expected(None));
    });
}

#[test]
fn default_user_can_write_is_true() {
    with_home(None, || {
        assert!(EnvDirs::new(PathBuf::from("/d")).user_can_write(std::path::Path::new("/any")));
    });
}
