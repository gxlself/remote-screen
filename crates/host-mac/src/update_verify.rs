//! Mac 自动更新：更新包解压、包结构与代码签名校验（06「自动更新」第 4 步）。

use std::collections::HashSet;
use std::ffi::{c_char, CString, OsStr, OsString};
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

use host_core::update::prepare::PackageVerifier;

pub const APP_BUNDLE_NAME: &str = "Remote Host.app";
pub const APP_IDENTIFIER: &str = "ai.gxlself.remotehost";
pub const AGENT_EXECUTABLE: &str = "rs-host-agent";

const OUTPUT_LEN: usize = 256;
const ERROR_LEN: usize = 512;
static VERIFY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

extern "C" {
    fn rs_codesign_team_self(
        out: *mut c_char,
        out_len: usize,
        err: *mut c_char,
        err_len: usize,
    ) -> i32;
    fn rs_codesign_team_path(
        path: *const c_char,
        out: *mut c_char,
        out_len: usize,
        err: *mut c_char,
        err_len: usize,
    ) -> i32;
    fn rs_codesign_check(
        path: *const c_char,
        requirement: *const c_char,
        err: *mut c_char,
        err_len: usize,
    ) -> i32;
}

/// 代码签名查询，测试里可替换。
pub trait CodeSigning: Send + Sync {
    /// 当前进程的 Team ID；未签名 / ad-hoc 为 Ok(None)。
    fn current_team_id(&self) -> Result<Option<String>, String>;
    /// 磁盘上代码的 Team ID（不做完整性校验）。
    fn team_id(&self, path: &Path) -> Result<Option<String>, String>;
    /// 完整性 + 可选 requirement 校验。
    fn check(&self, path: &Path, requirement: Option<&str>) -> Result<(), String>;
}

/// 真实实现：调用 codesign.m。
#[derive(Debug, Clone, Copy, Default)]
pub struct SecCodeSigning;

fn output_string(buffer: &[u8]) -> String {
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

fn ffi_error(buffer: &[u8], fallback: &str) -> String {
    let message = output_string(buffer);
    if message.is_empty() {
        fallback.to_owned()
    } else {
        message
    }
}

fn path_cstring(path: &Path) -> Result<CString, String> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "code path contains an embedded NUL".to_owned())
}

impl CodeSigning for SecCodeSigning {
    fn current_team_id(&self) -> Result<Option<String>, String> {
        let mut team = [0_u8; OUTPUT_LEN];
        let mut error = [0_u8; ERROR_LEN];
        // SAFETY: both output buffers are valid for the duration of the call;
        // the Objective-C shim writes at most the lengths supplied here.
        let result = unsafe {
            rs_codesign_team_self(
                team.as_mut_ptr().cast(),
                team.len(),
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        match result {
            1 => Ok(Some(output_string(&team))),
            0 => Ok(None),
            -1 => Err(ffi_error(&error, "could not query current code signature")),
            other => Err(format!(
                "code-signing query returned unexpected status {other}"
            )),
        }
    }

    fn team_id(&self, path: &Path) -> Result<Option<String>, String> {
        let path = path_cstring(path)?;
        let mut team = [0_u8; OUTPUT_LEN];
        let mut error = [0_u8; ERROR_LEN];
        // SAFETY: `path` is NUL-terminated and all output buffers remain
        // writable for the duration of the Objective-C call.
        let result = unsafe {
            rs_codesign_team_path(
                path.as_ptr(),
                team.as_mut_ptr().cast(),
                team.len(),
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        match result {
            1 => Ok(Some(output_string(&team))),
            0 => Ok(None),
            -1 => Err(ffi_error(&error, "could not query code signature")),
            other => Err(format!(
                "code-signing query returned unexpected status {other}"
            )),
        }
    }

    fn check(&self, path: &Path, requirement: Option<&str>) -> Result<(), String> {
        let path = path_cstring(path)?;
        let requirement = requirement
            .map(CString::new)
            .transpose()
            .map_err(|_| "code-signing requirement contains an embedded NUL".to_owned())?;
        let requirement_ptr = requirement
            .as_ref()
            .map_or(ptr::null(), |value| value.as_ptr());
        let mut error = [0_u8; ERROR_LEN];
        // SAFETY: both C strings and the error buffer stay alive and valid for
        // the entire Objective-C call; a null requirement is explicitly
        // supported by the shim.
        let result = unsafe {
            rs_codesign_check(
                path.as_ptr(),
                requirement_ptr,
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        match result {
            0 => Ok(()),
            -1 => Err(ffi_error(&error, "code-signing check failed")),
            other => Err(format!(
                "code-signing check returned unexpected status {other}"
            )),
        }
    }
}

/// Team ID 必须是 10 位大写字母或数字，否则 Err（防止拼进 requirement 时注入）。
pub fn app_requirement(team_id: &str) -> Result<String, String> {
    let bytes = team_id.as_bytes();
    if bytes.len() != 10
        || bytes
            .iter()
            .any(|byte| !byte.is_ascii_uppercase() && !byte.is_ascii_digit())
    {
        return Err("Team ID must be exactly 10 uppercase letters or digits".to_owned());
    }
    Ok(format!(
        "identifier \"{APP_IDENTIFIER}\" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"{team_id}\""
    ))
}

/// Compare the signing identity of the running app and an update candidate.
pub fn team_matches(current: Option<&str>, candidate: Option<&str>) -> Result<(), String> {
    let current = current
        .ok_or_else(|| "current app is not signed with a Team ID; refusing update".to_owned())?;
    let candidate = candidate.ok_or_else(|| "package is not signed with a Team ID".to_owned())?;
    if current != candidate {
        return Err("Team ID mismatch".to_owned());
    }
    Ok(())
}

fn regular_file(path: &Path, description: &str) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("{description} is missing: {error}"))?;
    if !metadata.file_type().is_file() {
        return Err(format!("{description} is not a regular file"));
    }
    Ok(())
}

fn real_directory(path: &Path, description: &str) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("{description} is missing: {error}"))?;
    if !metadata.file_type().is_dir() {
        return Err(format!("{description} is not a directory"));
    }
    Ok(())
}

/// Validate the one-app update archive layout and return its app bundle path.
pub fn validate_layout(dest: &Path) -> Result<PathBuf, String> {
    real_directory(dest, "extracted update directory")?;

    let entries = fs::read_dir(dest)
        .map_err(|error| format!("could not read extracted update directory: {error}"))?;
    let mut app = None;
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not read update entry: {error}"))?;
        if app.is_some() {
            return Err("update package must contain exactly one top-level entry".to_owned());
        }
        if entry.file_name() != OsStr::new(APP_BUNDLE_NAME) {
            return Err(format!(
                "update package top-level entry must be {APP_BUNDLE_NAME}"
            ));
        }
        let path = entry.path();
        real_directory(&path, "app bundle")?;
        app = Some(path);
    }

    let app = app.ok_or_else(|| "update package is empty".to_owned())?;
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    real_directory(&contents, "app Contents directory")?;
    real_directory(&macos, "app MacOS directory")?;
    regular_file(&contents.join("Info.plist"), "app Info.plist")?;
    regular_file(&macos.join(AGENT_EXECUTABLE), "app agent executable")?;
    Ok(app)
}

fn truncated_stderr(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr).chars().take(200).collect()
}

const ZIP_END_OF_CENTRAL_DIRECTORY: u32 = 0x0605_4b50;
const ZIP_CENTRAL_DIRECTORY_ENTRY: u32 = 0x0201_4b50;
const ZIP64_SENTINEL_U16: u16 = u16::MAX;
const ZIP64_SENTINEL_U32: u32 = u32::MAX;

struct ZipEntry {
    name: Vec<u8>,
    is_symlink: bool,
    uncompressed_size: usize,
}

fn zip_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let value = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([value[0], value[1]]))
}

fn zip_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let value = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn zip_entry_description(name: &[u8]) -> String {
    String::from_utf8_lossy(name).into_owned()
}

fn validate_zip_entry_name(name: &[u8]) -> Result<bool, String> {
    if name.is_empty() {
        return Err("update archive contains an empty entry name".to_owned());
    }
    if name.contains(&0) {
        return Err("update archive entry contains an embedded NUL".to_owned());
    }
    if name[0] == b'/' {
        return Err(format!(
            "update archive entry is an absolute path: {}",
            zip_entry_description(name)
        ));
    }

    let mut components = name.split(|byte| *byte == b'/').collect::<Vec<_>>();
    while components
        .last()
        .is_some_and(|component| component.is_empty())
    {
        components.pop();
    }
    if components.is_empty() || components.iter().any(|component| component.is_empty()) {
        return Err(format!(
            "update archive entry has an invalid path: {}",
            zip_entry_description(name)
        ));
    }

    let is_macosx_metadata = components[0] == b"__MACOSX";
    if components.iter().any(|component| *component == b"..") {
        return Err(format!(
            "update archive entry contains '..': {}",
            zip_entry_description(name)
        ));
    }
    if is_macosx_metadata {
        if components.len() == 1 && !name.ends_with(b"/") {
            return Err("update archive __MACOSX entry must be a directory".to_owned());
        }
        if components
            .get(1)
            .is_some_and(|component| *component != APP_BUNDLE_NAME.as_bytes())
        {
            return Err(format!(
                "update archive entry is outside {APP_BUNDLE_NAME}: {}",
                zip_entry_description(name)
            ));
        }
        return Ok(true);
    }
    if components[0] != APP_BUNDLE_NAME.as_bytes() {
        return Err(format!(
            "update archive entry is outside {APP_BUNDLE_NAME}: {}",
            zip_entry_description(name)
        ));
    }
    Ok(false)
}

fn resolve_path_within(root: &Path, parent: &Path, target: &Path) -> Result<PathBuf, String> {
    if target.is_absolute() {
        return Err("update archive contains a symlink with an absolute target".to_owned());
    }
    let relative_parent = parent
        .strip_prefix(root)
        .map_err(|_| "update archive symlink parent is outside the app bundle".to_owned())?;
    let mut resolved = root.to_path_buf();

    for component in relative_parent.components().chain(target.components()) {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(component) => resolved.push(component),
            std::path::Component::ParentDir => {
                if resolved == root || !resolved.pop() {
                    return Err("update archive symlink target escapes the app bundle".to_owned());
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err("update archive contains an absolute symlink target".to_owned());
            }
        }
    }
    Ok(resolved)
}

fn path_is_within(root: &Path, path: &Path) -> bool {
    path == root || path.strip_prefix(root).is_ok()
}

fn validate_symlink_target(root: &Path, link: &Path, target: &Path) -> Result<PathBuf, String> {
    if target.as_os_str().is_empty() {
        return Err("update archive contains a symlink with an empty target".to_owned());
    }
    if target.as_os_str().as_bytes().contains(&0) {
        return Err("update archive contains a symlink target with an embedded NUL".to_owned());
    }
    if link == root {
        return Err("app bundle entry must not be a symlink".to_owned());
    }
    let parent = link
        .parent()
        .ok_or_else(|| "update archive symlink has no parent".to_owned())?;
    let resolved = resolve_path_within(root, parent, target)?;
    if !path_is_within(root, &resolved) {
        return Err("update archive symlink target escapes the app bundle".to_owned());
    }
    Ok(resolved)
}

fn read_zip_symlink_target(zip: &Path, entry: &ZipEntry) -> Result<PathBuf, String> {
    if entry.uncompressed_size > 4096 {
        return Err("update archive symlink target is too long".to_owned());
    }
    if entry
        .name
        .iter()
        .any(|byte| matches!(*byte, b'*' | b'?' | b'[' | b']'))
    {
        return Err("update archive symlink name contains a zip wildcard".to_owned());
    }
    let entry_name = OsString::from_vec(entry.name.clone());
    let output = Command::new("/usr/bin/unzip")
        .arg("-p")
        .arg(zip)
        .arg(entry_name)
        .output()
        .map_err(|error| format!("could not read zip symlink target: {error}"))?;
    if !output.status.success() {
        let stderr = truncated_stderr(&output.stderr);
        let detail = if stderr.is_empty() {
            "no diagnostic".to_owned()
        } else {
            stderr
        };
        return Err(format!(
            "could not read zip symlink target ({}): {detail}",
            output.status
        ));
    }
    if output.stdout.len() > 4096 {
        return Err("update archive symlink target is too long".to_owned());
    }
    if output.stdout.len() != entry.uncompressed_size {
        return Err("update archive symlink target size does not match its entry".to_owned());
    }
    if output.stdout.is_empty() || output.stdout.contains(&0) {
        return Err("update archive contains an invalid symlink target".to_owned());
    }
    Ok(PathBuf::from(OsString::from_vec(output.stdout)))
}

/// Inspect every central-directory entry before invoking `ditto`, whose
/// extractor must never be the first component to interpret an untrusted path.
fn inspect_zip(zip: &Path) -> Result<Vec<ZipEntry>, String> {
    let bytes = fs::read(zip).map_err(|error| format!("could not read update zip: {error}"))?;
    if bytes.len() < 22 {
        return Err("update archive is missing its end-of-central-directory record".to_owned());
    }

    let minimum_eocd = bytes.len().saturating_sub(22 + usize::from(u16::MAX));
    let eocd = (minimum_eocd..=bytes.len() - 22)
        .rev()
        .find(|offset| {
            zip_u32(&bytes, *offset) == Some(ZIP_END_OF_CENTRAL_DIRECTORY)
                && zip_u16(&bytes, *offset + 20).is_some_and(|comment_len| {
                    *offset + 22 + usize::from(comment_len) <= bytes.len()
                })
        })
        .ok_or_else(|| "update archive has no valid end-of-central-directory record".to_owned())?;

    let disk = zip_u16(&bytes, eocd + 4).ok_or_else(|| "malformed zip disk number".to_owned())?;
    let central_disk = zip_u16(&bytes, eocd + 6)
        .ok_or_else(|| "malformed zip central directory disk".to_owned())?;
    let entries_on_disk =
        zip_u16(&bytes, eocd + 8).ok_or_else(|| "malformed zip entry count".to_owned())?;
    let total_entries =
        zip_u16(&bytes, eocd + 10).ok_or_else(|| "malformed zip entry count".to_owned())?;
    let central_size = zip_u32(&bytes, eocd + 12)
        .ok_or_else(|| "malformed zip central directory size".to_owned())?;
    let central_offset = zip_u32(&bytes, eocd + 16)
        .ok_or_else(|| "malformed zip central directory offset".to_owned())?;
    if disk != 0
        || central_disk != 0
        || entries_on_disk != total_entries
        || total_entries == ZIP64_SENTINEL_U16
        || central_size == ZIP64_SENTINEL_U32
        || central_offset == ZIP64_SENTINEL_U32
    {
        return Err("multi-disk and ZIP64 update archives are not supported".to_owned());
    }

    let central_start = usize::try_from(central_offset)
        .map_err(|_| "zip central directory offset is too large".to_owned())?;
    let central_len = usize::try_from(central_size)
        .map_err(|_| "zip central directory size is too large".to_owned())?;
    let central_end = central_start
        .checked_add(central_len)
        .ok_or_else(|| "zip central directory range overflowed".to_owned())?;
    if central_end > eocd || central_end > bytes.len() {
        return Err("zip central directory is outside the archive".to_owned());
    }

    let mut entries = Vec::with_capacity(usize::from(total_entries));
    let mut seen = HashSet::with_capacity(usize::from(total_entries));
    let mut offset = central_start;
    for _ in 0..total_entries {
        if offset.checked_add(46).is_none_or(|end| end > central_end)
            || zip_u32(&bytes, offset) != Some(ZIP_CENTRAL_DIRECTORY_ENTRY)
        {
            return Err("malformed zip central directory entry".to_owned());
        }
        let name_len = usize::from(
            zip_u16(&bytes, offset + 28)
                .ok_or_else(|| "malformed zip entry name length".to_owned())?,
        );
        let extra_len = usize::from(
            zip_u16(&bytes, offset + 30)
                .ok_or_else(|| "malformed zip entry extra length".to_owned())?,
        );
        let comment_len = usize::from(
            zip_u16(&bytes, offset + 32)
                .ok_or_else(|| "malformed zip entry comment length".to_owned())?,
        );
        let entry_len = 46usize
            .checked_add(name_len)
            .and_then(|length| length.checked_add(extra_len))
            .and_then(|length| length.checked_add(comment_len))
            .ok_or_else(|| "zip central directory entry range overflowed".to_owned())?;
        let entry_end = offset
            .checked_add(entry_len)
            .ok_or_else(|| "zip central directory entry range overflowed".to_owned())?;
        if entry_end > central_end {
            return Err("zip central directory entry exceeds its declared range".to_owned());
        }

        let name = bytes[offset + 46..offset + 46 + name_len].to_vec();
        let is_macosx_metadata = validate_zip_entry_name(&name)?;
        if !seen.insert(name.clone()) {
            return Err(format!(
                "update archive contains a duplicate entry: {}",
                zip_entry_description(&name)
            ));
        }

        let external_attributes = zip_u32(&bytes, offset + 38)
            .ok_or_else(|| "malformed zip file attributes".to_owned())?;
        let uncompressed_size = usize::try_from(
            zip_u32(&bytes, offset + 24)
                .ok_or_else(|| "malformed zip uncompressed size".to_owned())?,
        )
        .map_err(|_| "zip entry is too large".to_owned())?;
        let unix_mode = external_attributes >> 16;
        let is_symlink = unix_mode & 0o170000 == 0o120000;
        if is_macosx_metadata && is_symlink {
            return Err(format!(
                "update archive __MACOSX entry must not be a symlink: {}",
                zip_entry_description(&name)
            ));
        }
        entries.push(ZipEntry {
            name,
            is_symlink,
            uncompressed_size,
        });
        offset = entry_end;
    }
    if offset != central_end {
        return Err("zip central directory has trailing bytes".to_owned());
    }

    let root = Path::new(APP_BUNDLE_NAME);
    for entry in &entries {
        if !entry.is_symlink {
            continue;
        }
        let link = PathBuf::from(OsString::from_vec(entry.name.clone()));
        let target = read_zip_symlink_target(zip, entry)?;
        validate_symlink_target(root, &link, &target)?;
    }
    Ok(entries)
}

fn clear_destination(dest: &Path) -> Result<(), String> {
    let entries = match fs::read_dir(dest) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("could not inspect extraction destination: {error}")),
    };
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("could not inspect extraction destination: {error}"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("could not inspect extracted entry: {error}"))?;
        if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
            fs::remove_dir_all(&path)
                .map_err(|error| format!("could not remove extracted directory: {error}"))?;
        } else {
            fs::remove_file(&path)
                .map_err(|error| format!("could not remove extracted entry: {error}"))?;
        }
    }
    Ok(())
}

fn validate_extracted_symlinks(bundle: &Path) -> Result<(), String> {
    let root = fs::canonicalize(bundle)
        .map_err(|error| format!("could not resolve extracted app bundle: {error}"))?;
    fn visit(path: &Path, root: &Path) -> Result<(), String> {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("could not inspect extracted entry: {error}"))?;
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(path)
                .map_err(|error| format!("could not read extracted symlink: {error}"))?;
            let resolved = validate_symlink_target(root, path, &target)?;
            match fs::canonicalize(&resolved) {
                Ok(canonical) if !path_is_within(root, &canonical) => {
                    return Err("extracted symlink target escapes the app bundle".to_owned());
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "could not resolve extracted symlink target: {error}"
                    ));
                }
            }
            return Ok(());
        }
        if !metadata.file_type().is_dir() {
            return Ok(());
        }
        for entry in fs::read_dir(path)
            .map_err(|error| format!("could not read extracted app bundle: {error}"))?
        {
            let entry =
                entry.map_err(|error| format!("could not read extracted entry: {error}"))?;
            visit(&entry.path(), root)?;
        }
        Ok(())
    }

    visit(&root, &root)
}

/// Extract an update zip into an empty destination and validate its layout.
pub fn extract_update_zip(zip: &Path, dest: &Path) -> Result<PathBuf, String> {
    inspect_zip(zip)?;
    match fs::symlink_metadata(dest) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                return Err("update extraction destination is not a directory".to_owned());
            }
            let mut entries = fs::read_dir(dest)
                .map_err(|error| format!("could not inspect extraction destination: {error}"))?;
            if let Some(entry) = entries.next() {
                entry.map_err(|error| {
                    format!("could not inspect extraction destination: {error}")
                })?;
                return Err("update extraction destination must be empty".to_owned());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(dest)
                .map_err(|error| format!("could not create extraction destination: {error}"))?;
            fs::set_permissions(dest, fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("could not secure extraction destination: {error}"))?;
        }
        Err(error) => {
            return Err(format!("could not inspect extraction destination: {error}"));
        }
    }

    let result = (|| {
        let output = Command::new("/usr/bin/ditto")
            .arg("-x")
            .arg("-k")
            .arg(zip)
            .arg(dest)
            .output()
            .map_err(|error| format!("could not start /usr/bin/ditto: {error}"))?;
        if !output.status.success() {
            let stderr = truncated_stderr(&output.stderr);
            let detail = if stderr.is_empty() {
                "no diagnostic".to_owned()
            } else {
                stderr
            };
            return Err(format!("ditto failed ({}): {detail}", output.status));
        }
        let app = validate_layout(dest)?;
        validate_extracted_symlinks(&app)?;
        Ok(app)
    })();
    if let Err(error) = &result {
        if let Err(cleanup) = clear_destination(dest) {
            return Err(format!("{error}; {cleanup}"));
        }
    }
    result
}

/// Verify the candidate app's Team ID and complete code signature.
pub fn verify_app(signing: &dyn CodeSigning, app: &Path, team_id: &str) -> Result<(), String> {
    let candidate = signing.team_id(app)?;
    team_matches(Some(team_id), candidate.as_deref())?;
    let requirement = app_requirement(team_id)?;
    signing.check(app, Some(&requirement))
}

/// macOS update package verifier retaining the app Team ID observed at start.
pub struct MacCodeVerifier<C: CodeSigning = SecCodeSigning> {
    current: Option<String>,
    signing: C,
}

impl MacCodeVerifier<SecCodeSigning> {
    pub fn new() -> Self {
        let signing = SecCodeSigning;
        let current = signing.current_team_id().ok().flatten();
        Self { current, signing }
    }
}

impl Default for MacCodeVerifier<SecCodeSigning> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: CodeSigning> MacCodeVerifier<C> {
    pub fn with_signing(current: Option<String>, signing: C) -> Self {
        Self { current, signing }
    }
}

fn remove_dir_if_present(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not remove verification directory: {error}")),
    }
}

impl<C: CodeSigning> PackageVerifier for MacCodeVerifier<C> {
    fn verify_platform(&self, package: &Path) -> Result<(), String> {
        let Some(current) = self.current.as_deref() else {
            let error = "current app is not signed with a Team ID; refusing update".to_owned();
            tracing::warn!(event = "upgrade.verify", result = "error", reason = %error);
            return Err(error);
        };

        let parent = package
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let sequence = VERIFY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!("verify-{}-{sequence}", process::id()));

        let result = if let Err(error) = remove_dir_if_present(&temporary) {
            Err(error)
        } else {
            (|| {
                let app = extract_update_zip(package, &temporary)?;
                verify_app(&self.signing, &app, current)
            })()
        };

        if let Err(error) = remove_dir_if_present(&temporary) {
            tracing::warn!(
                event = "upgrade.verify",
                result = "cleanup_error",
                reason = %error
            );
        }
        match &result {
            Ok(()) => tracing::info!(event = "upgrade.verify", result = "ok"),
            Err(error) => tracing::warn!(
                event = "upgrade.verify",
                result = "error",
                reason = %error
            ),
        }
        result
    }
}
