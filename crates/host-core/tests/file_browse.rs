use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use host_core::file_browse::{
    expand_home, list_dir, resolve_dest_dir, resolve_dir, resolve_file, BrowseError, BrowsePolicy,
    ListOptions,
};
use rsp_core::pb::{file_entry::Kind, FileEntry, FileError};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDir {
    base: PathBuf,
    home: PathBuf,
}

impl TestDir {
    fn new() -> Self {
        let serial = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock is after the epoch")
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "rs-file-browse-{}-{nanos}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(base.join("home")).expect("test home is created");
        let base = fs::canonicalize(base).expect("test directory is canonical");
        let home = base.join("home");
        Self { base, home }
    }

    fn policy(&self) -> BrowsePolicy {
        BrowsePolicy::new(&[], &self.home, &[])
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.home.join(name);
        fs::create_dir_all(&path).expect("test directory is created");
        path
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.home.join(name);
        fs::write(&path, bytes).expect("test file is created");
        path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.base).expect("test directory is removed");
    }
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("test path is UTF-8")
}

fn names(entries: &[FileEntry]) -> Vec<&str> {
    entries.iter().map(|entry| entry.name.as_str()).collect()
}

fn entry<'a>(entries: &'a [FileEntry], name: &str) -> &'a FileEntry {
    entries
        .iter()
        .find(|entry| entry.name == name)
        .expect("expected entry is listed")
}

fn count_options() -> ListOptions {
    ListOptions {
        count_budget: Duration::from_secs(30),
    }
}

#[test]
fn expand_home_only_expands_tilde_and_slash_prefix() {
    let temp = TestDir::new();
    let home = &temp.home;
    assert_eq!(expand_home("~", home), *home);
    assert_eq!(expand_home("~/a", home), home.join("a"));
    assert_eq!(expand_home("/x", home), PathBuf::from("/x"));
    assert_eq!(expand_home("~bob", home), PathBuf::from("~bob"));
    assert_eq!(expand_home("~bob/a", home), PathBuf::from("~bob/a"));
}

#[test]
fn empty_directory_path_selects_home_without_parent() {
    let temp = TestDir::new();
    let p = temp.policy();
    assert_eq!(p.roots, vec![temp.home.clone()]);
    assert!(!p.show_hidden);
    let dir = resolve_dir(&p, "").expect("default root resolves");
    assert_eq!(dir.path, temp.home);
    assert_eq!(dir.parent, None);
    assert_eq!(
        resolve_dir(&p, path_str(&temp.home))
            .expect("explicit root resolves")
            .parent,
        None
    );
    assert_eq!(
        ListOptions::default().count_budget,
        Duration::from_millis(200)
    );
}

#[test]
fn child_directory_has_canonical_parent() {
    let temp = TestDir::new();
    let sub = temp.dir("sub");
    let dir = resolve_dir(&temp.policy(), path_str(&sub.join("../sub")))
        .expect("in-root parent components resolve");
    assert_eq!(dir.path, sub);
    assert_eq!(dir.parent, Some(temp.home.clone()));
}

#[test]
fn relative_paths_are_rejected_by_all_resolvers() {
    let temp = TestDir::new();
    temp.dir("sub");
    let p = temp.policy();
    assert!(matches!(
        resolve_dir(&p, "sub"),
        Err(BrowseError::OutsideRoot)
    ));
    assert!(matches!(
        resolve_file(&p, "sub"),
        Err(BrowseError::OutsideRoot)
    ));
    assert!(matches!(
        resolve_dest_dir(&p, "sub"),
        Err(BrowseError::OutsideRoot)
    ));
}

#[test]
fn parent_components_cannot_escape_home() {
    let temp = TestDir::new();
    fs::create_dir(temp.base.join("outside")).expect("outside directory is created");
    let outside = temp.home.join("../outside");
    assert!(matches!(
        resolve_dir(&temp.policy(), path_str(&outside)),
        Err(BrowseError::OutsideRoot)
    ));
}

#[test]
fn root_comparison_uses_path_components() {
    let temp = TestDir::new();
    let a = temp.dir("a");
    let ab = temp.dir("ab");
    let p = BrowsePolicy::new(&[path_str(&a).to_owned()], &temp.home, &[]);
    assert!(matches!(
        resolve_dir(&p, path_str(&ab)),
        Err(BrowseError::OutsideRoot)
    ));
}

#[test]
fn every_configured_root_has_no_parent() {
    let temp = TestDir::new();
    let nested = temp.dir("nested");
    let other = temp.base.join("other");
    fs::create_dir(&other).expect("second root is created");
    let p = BrowsePolicy::new(
        &[
            path_str(&temp.home).to_owned(),
            path_str(&nested).to_owned(),
            path_str(&other).to_owned(),
        ],
        &temp.home,
        &[],
    );
    for root in &p.roots {
        let dir = resolve_dir(&p, path_str(root)).expect("configured root resolves");
        assert_eq!(dir.path, *root);
        assert_eq!(dir.parent, None);
    }
    assert_eq!(
        resolve_dir(&p, "").expect("first root resolves").path,
        temp.home
    );
}

#[test]
fn extra_deny_blocks_directory_files_and_listing() {
    let temp = TestDir::new();
    let state = temp.dir("state");
    let key = temp.file("state/key", b"test fixture");
    temp.dir("state-other");
    let p = BrowsePolicy::new(&[], &temp.home, std::slice::from_ref(&state));
    assert_eq!(p.deny, vec![state.clone()]);
    assert!(matches!(
        resolve_dir(&p, path_str(&state)),
        Err(BrowseError::OutsideRoot)
    ));
    assert!(matches!(
        resolve_file(&p, path_str(&key)),
        Err(BrowseError::OutsideRoot)
    ));
    assert!(matches!(
        resolve_dest_dir(&p, path_str(&state)),
        Err(BrowseError::OutsideRoot)
    ));
    assert_eq!(
        names(&list_dir(&p, &temp.home, &count_options()).expect("root lists")),
        ["state-other"]
    );
}

#[test]
fn existing_default_deny_directories_are_blocked_even_when_hidden_are_shown() {
    let temp = TestDir::new();
    for name in ["Library", ".ssh", ".gnupg", ".Trash"] {
        temp.dir(name);
    }
    let mut p = temp.policy();
    assert_eq!(p.deny.len(), 4);
    p.show_hidden = true;
    for path in &p.deny {
        assert!(matches!(
            resolve_dir(&p, path_str(path)),
            Err(BrowseError::OutsideRoot)
        ));
    }
    assert!(list_dir(&p, &temp.home, &count_options())
        .expect("root lists")
        .is_empty());
}

#[test]
fn resolvers_require_existing_paths_of_the_expected_type() {
    let temp = TestDir::new();
    let dir = temp.dir("sub");
    let file = temp.file("file", b"contents");
    let missing = temp.home.join("missing");
    let p = temp.policy();
    assert!(matches!(
        resolve_file(&p, path_str(&dir)),
        Err(BrowseError::NotAFile)
    ));
    assert!(matches!(
        resolve_dir(&p, path_str(&file)),
        Err(BrowseError::NotADir)
    ));
    assert!(matches!(
        resolve_dest_dir(&p, path_str(&file)),
        Err(BrowseError::NotADir)
    ));
    assert!(matches!(
        resolve_file(&p, path_str(&missing)),
        Err(BrowseError::NotFound)
    ));
    assert!(matches!(
        resolve_dir(&p, path_str(&missing)),
        Err(BrowseError::NotFound)
    ));
    assert!(matches!(
        resolve_dest_dir(&p, path_str(&missing)),
        Err(BrowseError::NotFound)
    ));
    assert!(matches!(resolve_file(&p, ""), Err(BrowseError::NotFound)));
    assert!(matches!(
        resolve_dest_dir(&p, ""),
        Err(BrowseError::NotFound)
    ));
    assert_eq!(
        resolve_file(&p, path_str(&file)).expect("file resolves"),
        file
    );
    assert_eq!(
        resolve_dest_dir(&p, path_str(&dir)).expect("destination resolves"),
        dir
    );
    assert!(matches!(
        resolve_dir(&p, path_str(&file.join("child"))),
        Err(BrowseError::NotADir)
    ));
}

#[test]
fn listings_sort_directories_then_case_folded_names_with_original_name_tiebreak() {
    let temp = TestDir::new();
    temp.dir("B");
    temp.file("a.txt", b"a");
    temp.file("C.txt", b"ccc");
    temp.dir("b");
    temp.file(".hidden", b"hidden");
    let mut p = temp.policy();
    let entries = list_dir(&p, &temp.home, &count_options()).expect("root lists");
    assert_eq!(names(&entries), ["B", "b", "a.txt", "C.txt"]);
    assert_eq!(entry(&entries, "B").kind(), Kind::Dir);
    assert_eq!(entry(&entries, "B").size, 0);
    assert_eq!(entry(&entries, "C.txt").size, 3);
    p.show_hidden = true;
    let entries = list_dir(&p, &temp.home, &count_options()).expect("hidden entries list");
    assert_eq!(names(&entries), ["B", "b", ".hidden", "a.txt", "C.txt"]);
}

#[test]
fn sorting_uses_unicode_lowercase() {
    let temp = TestDir::new();
    for name in ["Éclair", "éclair", "z", "Ångström", "ångström"] {
        temp.file(name, b"");
    }
    let entries = list_dir(&temp.policy(), &temp.home, &count_options()).expect("root lists");
    assert_eq!(
        names(&entries),
        ["z", "Ångström", "ångström", "Éclair", "éclair"]
    );
}

#[test]
fn child_counts_are_only_for_directories_and_zero_budget_disables_them() {
    let temp = TestDir::new();
    temp.dir("sub");
    temp.dir("empty");
    for name in ["sub/one", "sub/two", "sub/.hidden"] {
        temp.file(name, b"");
    }
    temp.file("file", b"data");
    let p = temp.policy();
    let entries = list_dir(&p, &temp.home, &count_options()).expect("root lists");
    assert_eq!(entry(&entries, "sub").child_count, 3);
    assert_eq!(entry(&entries, "empty").child_count, 0);
    assert_eq!(entry(&entries, "file").child_count, -1);
    let entries = list_dir(
        &p,
        &temp.home,
        &ListOptions {
            count_budget: Duration::ZERO,
        },
    )
    .expect("zero-budget root lists");
    assert!(entries.iter().all(|entry| entry.child_count == -1));
}

#[test]
fn child_count_is_capped_at_ten_thousand() {
    let temp = TestDir::new();
    let sub = temp.dir("sub");
    for index in 0..10_001 {
        fs::write(sub.join(index.to_string()), b"").expect("child is created");
    }
    let entries = list_dir(&temp.policy(), &temp.home, &count_options()).expect("root lists");
    assert_eq!(entry(&entries, "sub").child_count, 10_000);
}

#[test]
fn invalid_roots_are_discarded_and_all_invalid_fall_back_to_home() {
    let temp = TestDir::new();
    let existing = temp.dir("existing");
    let missing = path_str(&temp.home.join("missing")).to_owned();
    let p = BrowsePolicy::new(&[missing.clone(), "~/existing".into()], &temp.home, &[]);
    assert_eq!(p.roots, vec![existing]);
    let p = BrowsePolicy::new(&[missing], &temp.home, &[]);
    assert_eq!(p.roots, vec![temp.home.clone()]);
    assert_eq!(
        resolve_dir(&p, "").expect("fallback root resolves").path,
        temp.home
    );
}

#[test]
fn listing_missing_paths_and_files_returns_mapped_errors() {
    let temp = TestDir::new();
    let file = temp.file("file", b"");
    let p = temp.policy();
    assert!(matches!(
        list_dir(&p, &temp.home.join("missing"), &count_options()),
        Err(BrowseError::NotFound)
    ));
    assert!(matches!(
        list_dir(&p, &file, &count_options()),
        Err(BrowseError::NotADir)
    ));
}

#[test]
fn every_browse_error_maps_to_the_protocol() {
    for (error, expected) in [
        (BrowseError::NotFound, FileError::NotFound),
        (BrowseError::PermissionDenied, FileError::PermissionDenied),
        (BrowseError::OutsideRoot, FileError::OutsideRoot),
        (BrowseError::NotADir, FileError::NotADir),
        (BrowseError::NotAFile, FileError::NotAFile),
        (
            BrowseError::Io(io::Error::from(io::ErrorKind::Other)),
            FileError::Io,
        ),
    ] {
        assert_eq!(error.to_pb(), expected);
    }
}

#[test]
fn io_errors_are_classified_when_mapping_to_the_protocol() {
    for (kind, expected) in [
        (io::ErrorKind::NotFound, FileError::NotFound),
        (io::ErrorKind::PermissionDenied, FileError::PermissionDenied),
        (io::ErrorKind::NotADirectory, FileError::NotADir),
        (io::ErrorKind::IsADirectory, FileError::NotAFile),
        (io::ErrorKind::StorageFull, FileError::NoSpace),
        (io::ErrorKind::InvalidInput, FileError::Io),
    ] {
        let error: BrowseError = io::Error::from(kind).into();
        assert_eq!(error.to_pb(), expected);
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::os::unix::net::UnixListener;

    struct RestorePermissions {
        path: PathBuf,
        permissions: fs::Permissions,
    }

    impl RestorePermissions {
        fn set(path: &Path, mode: u32) -> Self {
            let permissions = fs::metadata(path)
                .expect("permissions are readable")
                .permissions();
            fs::set_permissions(path, fs::Permissions::from_mode(mode))
                .expect("test permissions are set");
            Self {
                path: path.to_owned(),
                permissions,
            }
        }
    }

    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            fs::set_permissions(&self.path, self.permissions.clone())
                .expect("test permissions are restored");
        }
    }

    #[test]
    fn external_symlinks_cannot_resolve_and_are_listed_without_target_metadata() {
        let temp = TestDir::new();
        let outside = temp.base.join("outside");
        fs::create_dir(&outside).expect("outside directory is created");
        fs::write(outside.join("file"), b"outside data").expect("outside file is created");
        let link = temp.home.join("external");
        symlink(&outside, &link).expect("external directory symlink is created");
        symlink(outside.join("file"), temp.home.join("external-file"))
            .expect("external file symlink is created");
        let p = temp.policy();
        assert!(matches!(
            resolve_dir(&p, path_str(&link)),
            Err(BrowseError::OutsideRoot)
        ));
        assert!(matches!(
            resolve_dest_dir(&p, path_str(&link)),
            Err(BrowseError::OutsideRoot)
        ));
        assert!(matches!(
            resolve_file(&p, path_str(&temp.home.join("external-file"))),
            Err(BrowseError::OutsideRoot)
        ));
        let entries = list_dir(&p, &temp.home, &count_options()).expect("root lists");
        for item in &entries {
            assert_eq!(item.kind(), Kind::Symlink);
            assert_eq!(item.size, 0);
            assert_eq!(item.child_count, -1);
        }
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn internal_symlinks_use_target_type_size_mtime_and_child_count() {
        let temp = TestDir::new();
        let target = temp.file("target", b"target contents");
        let target_time = UNIX_EPOCH + Duration::from_micros(1_700_000_000_123_456);
        fs::File::open(&target)
            .expect("target opens")
            .set_times(fs::FileTimes::new().set_modified(target_time))
            .expect("target mtime is set");
        symlink(&target, temp.home.join("link")).expect("file symlink is created");
        let sub = temp.dir("sub");
        temp.file("sub/child", b"");
        symlink(&sub, temp.home.join("dir-link")).expect("directory symlink is created");
        let p = temp.policy();
        assert_eq!(
            resolve_file(&p, path_str(&temp.home.join("link"))).expect("link resolves"),
            target
        );
        let dir = resolve_dir(&p, path_str(&temp.home.join("dir-link")))
            .expect("directory link resolves");
        assert_eq!(dir.path, sub);
        assert_eq!(dir.parent, Some(temp.home.clone()));
        let entries = list_dir(&p, &temp.home, &count_options()).expect("root lists");
        let link = entry(&entries, "link");
        assert_eq!(link.kind(), Kind::File);
        assert_eq!(link.size, b"target contents".len() as u64);
        assert_eq!(link.mtime_us, 1_700_000_000_123_456);
        assert_eq!(link.mtime_us, entry(&entries, "target").mtime_us);
        assert_eq!(link.child_count, -1);
        let dir_link = entry(&entries, "dir-link");
        assert_eq!(dir_link.kind(), Kind::Dir);
        assert_eq!(dir_link.size, 0);
        assert_eq!(dir_link.child_count, 1);
        assert_eq!(names(&entries), ["dir-link", "sub", "link", "target"]);
    }

    #[test]
    fn broken_and_looping_symlinks_remain_symlink_entries() {
        let temp = TestDir::new();
        symlink("missing", temp.home.join("broken")).expect("broken link is created");
        symlink("loop", temp.home.join("loop")).expect("looping link is created");
        let p = temp.policy();
        assert!(matches!(
            resolve_file(&p, path_str(&temp.home.join("broken"))),
            Err(BrowseError::NotFound)
        ));
        let entries = list_dir(&p, &temp.home, &count_options()).expect("root lists");
        assert_eq!(names(&entries), ["broken", "loop"]);
        assert!(entries
            .iter()
            .all(|entry| entry.kind() == Kind::Symlink && entry.size == 0));
    }

    #[test]
    fn symlinks_to_denied_targets_are_hidden_and_cannot_resolve() {
        let temp = TestDir::new();
        let state = temp.dir("state");
        let key = temp.file("state/key", b"fixture");
        symlink(&state, temp.home.join("state-link")).expect("state symlink is created");
        symlink(&key, temp.home.join("key-link")).expect("key symlink is created");
        let mut p = BrowsePolicy::new(&[], &temp.home, &[state]);
        p.show_hidden = true;
        assert!(matches!(
            resolve_dir(&p, path_str(&temp.home.join("state-link"))),
            Err(BrowseError::OutsideRoot)
        ));
        assert!(matches!(
            resolve_file(&p, path_str(&temp.home.join("key-link"))),
            Err(BrowseError::OutsideRoot)
        ));
        assert!(list_dir(&p, &temp.home, &count_options())
            .expect("root lists")
            .is_empty());
    }

    #[test]
    fn configured_roots_and_denies_are_canonicalized() {
        let temp = TestDir::new();
        let root = temp.dir("root");
        let state = temp.dir("root/state");
        symlink(&root, temp.home.join("root-link")).expect("root link is created");
        symlink(&state, temp.home.join("state-link")).expect("state link is created");
        let p = BrowsePolicy::new(
            &[path_str(&temp.home.join("root-link")).to_owned()],
            &temp.home,
            &[temp.home.join("state-link"), temp.home.join("missing")],
        );
        assert_eq!(p.roots, vec![root.clone()]);
        assert_eq!(p.deny, vec![state]);
        assert_eq!(
            resolve_dir(&p, "")
                .expect("canonical default resolves")
                .path,
            root
        );
    }

    #[test]
    fn readonly_destination_is_denied() {
        let temp = TestDir::new();
        let dir = temp.dir("readonly");
        let _permissions = RestorePermissions::set(&dir, 0o555);
        assert!(matches!(
            resolve_dest_dir(&temp.policy(), path_str(&dir)),
            Err(BrowseError::PermissionDenied)
        ));
        assert!(resolve_dir(&temp.policy(), path_str(&dir)).is_ok());
    }

    #[test]
    fn unreadable_directory_returns_permission_denied_and_unknown_child_count() {
        let temp = TestDir::new();
        let dir = temp.dir("unreadable");
        let p = temp.policy();
        let _permissions = RestorePermissions::set(&dir, 0o000);
        match fs::read_dir(&dir) {
            Ok(_) => {
                eprintln!("skipping chmod 000 permission assertions: current user can read the directory (for example root)");
                return;
            }
            Err(error) => assert_eq!(error.kind(), io::ErrorKind::PermissionDenied),
        }
        assert!(matches!(
            list_dir(&p, &dir, &count_options()),
            Err(BrowseError::PermissionDenied)
        ));
        let entries = list_dir(&p, &temp.home, &count_options()).expect("parent lists");
        assert_eq!(entry(&entries, "unreadable").kind(), Kind::Dir);
        assert_eq!(entry(&entries, "unreadable").child_count, -1);
    }

    #[test]
    fn non_utf8_filenames_are_skipped() {
        let temp = TestDir::new();
        fs::write(
            temp.home.join(OsStr::from_bytes(b"\xff.bin")),
            b"invalid name",
        )
        .expect("non-UTF-8 file is created");
        temp.file("valid", b"");
        let entries = list_dir(&temp.policy(), &temp.home, &count_options()).expect("root lists");
        assert_eq!(names(&entries), ["valid"]);
    }

    #[test]
    fn socket_entries_are_other_and_are_not_regular_files() {
        let temp = TestDir::new();
        let socket_path = temp.home.join("socket");
        // A directory fd avoids sun_path's limit with a long Linux TMPDIR.
        #[cfg(target_os = "linux")]
        let socket_dir = fs::File::open(&temp.home).expect("socket directory opens");
        #[cfg(target_os = "linux")]
        let bind_path = {
            use std::os::fd::AsRawFd;
            PathBuf::from(format!("/proc/self/fd/{}/socket", socket_dir.as_raw_fd()))
        };
        #[cfg(not(target_os = "linux"))]
        let bind_path = socket_path.clone();
        let listener = UnixListener::bind(bind_path);
        let _listener = match listener {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
                eprintln!("skipping Unix socket fixture: temporary path exceeds sun_path limit");
                return;
            }
            Err(error) => panic!("socket fixture binds: {error}"),
        };
        let p = temp.policy();
        assert!(matches!(
            resolve_file(&p, path_str(&socket_path)),
            Err(BrowseError::NotAFile)
        ));
        let entries = list_dir(&p, &temp.home, &count_options()).expect("root lists");
        let socket = entry(&entries, "socket");
        assert_eq!(socket.kind(), Kind::Other);
        assert_eq!(socket.size, 0);
        assert_eq!(socket.child_count, -1);
    }
}
