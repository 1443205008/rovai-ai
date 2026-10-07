//! Atomic native-file edits follow symlinks without replacing the user's links.
use super::native;
use anyhow::{Result, ensure};
use std::path::{Path, PathBuf};

pub(crate) fn target(path: &Path) -> Result<PathBuf> {
    fn resolve(path: &Path, links: usize) -> Result<PathBuf> {
        ensure!(links < 40, "原生配置符号链接循环或层级过深。");
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let link = std::fs::read_link(path)?;
                let next = if link.is_absolute() {
                    link
                } else {
                    path.parent().unwrap_or(Path::new(".")).join(link)
                };
                resolve(&next, links + 1)
            }
            Ok(_) => Ok(std::fs::canonicalize(path)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let parent = path
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("原生配置没有可写目标目录。"))?;
                let name = path
                    .file_name()
                    .ok_or_else(|| anyhow::anyhow!("原生配置目标路径无效。"))?;
                Ok(resolve(parent, links)?.join(name))
            }
            Err(error) => Err(error.into()),
        }
    }
    resolve(path, 0).map_err(|_| anyhow::anyhow!("无法解析原生配置目标：{}", path.display()))
}

pub(crate) struct NativeFile {
    path: PathBuf,
    target: PathBuf,
    before: Option<Vec<u8>>,
}
impl NativeFile {
    pub(crate) fn read(path: &Path) -> Result<Self> {
        let target = target(path)?;
        let before = native::read_bytes(&target)?;
        Ok(Self {
            path: path.into(),
            target,
            before,
        })
    }
    pub(crate) fn current(&self) -> Result<Option<Vec<u8>>> {
        ensure!(
            target(&self.path)? == self.target,
            "原生配置符号链接目标已变化，草稿已保留，请再次保存。"
        );
        native::read_bytes(&self.target)
    }
    pub(crate) fn matches(&self, bytes: &Option<Vec<u8>>) -> Result<bool> {
        Ok(self.current()? == *bytes)
    }
    pub(crate) fn unchanged(&self) -> Result<()> {
        ensure!(
            self.matches(&self.before)?,
            "原生文件在保存期间变化，草稿已保留，请再次保存。"
        );
        Ok(())
    }
    pub(crate) fn write(&self, bytes: &[u8]) -> Result<()> {
        atomic_write(&self.target, bytes, || {
            self.unchanged()?;
            ensure!(
                !std::fs::metadata(&self.target)
                    .is_ok_and(|metadata| metadata.permissions().readonly()),
                "原生配置来源为只读，未修改：{}。请在该来源调整写入权限后重试，草稿已保留。",
                self.path.display()
            );
            Ok(())
        })
    }
    pub(crate) fn restore(&self, expected: &Option<Vec<u8>>) -> Result<()> {
        // Roll back only our own bytes on the captured physical target. A retargeted
        // link must never cause rollback to overwrite a different file.
        let unchanged = || {
            ensure!(
                target(&self.target)? == self.target
                    && native::read_bytes(&self.target)? == *expected,
                "原生目标随后又被外部修改，未覆盖外部变化；草稿已保留，请检查原生来源。"
            );
            Ok(())
        };
        match &self.before {
            Some(bytes) => atomic_write(&self.target, bytes, unchanged),
            None => {
                unchanged()?;
                match std::fs::remove_file(&self.target) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error.into()),
                }
            }
        }
    }
}

#[cfg(unix)]
fn atomic_write(path: &Path, bytes: &[u8], unchanged: impl FnOnce() -> Result<()>) -> Result<()> {
    use std::{
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    };
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("原生配置没有目标目录。"))?;
    // A linked native config may live in a user's shared dotfiles directory.
    // Restrict new files/directories without chmod-ing an existing parent.
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        unchanged()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn atomic_write(path: &Path, bytes: &[u8], unchanged: impl FnOnce() -> Result<()>) -> Result<()> {
    atomic_write_native_bytes(path, bytes, unchanged)
}

#[cfg(windows)]
fn atomic_write_native_bytes(
    path: &Path,
    bytes: &[u8],
    unchanged: impl FnOnce() -> Result<()>,
) -> Result<()> {
    use anyhow::Context;
    use std::{io::Write, os::windows::ffi::OsStrExt, ptr::null};
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_WRITE_THROUGH, MoveFileExW, ReplaceFileW,
    };

    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("原生配置没有目标目录。"))?;
    // Native files belong to the user/CLI. Let Windows enforce their ordinary
    // permissions; neither the parent nor the staging file gets a private DACL.
    std::fs::create_dir_all(parent)?;
    let existed = path.try_exists()?;
    let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let backup = temporary.with_extension("bak");
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>()
    };
    let target_wide = wide(path);
    let temporary_wide = wide(&temporary);
    let backup_wide = wide(&backup);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        unchanged()?;
        // SAFETY: all paths are NUL-terminated and live throughout the call;
        // the staging handle is closed. Zero flags preserve the target's DACL.
        let published = unsafe {
            if existed {
                ReplaceFileW(
                    target_wide.as_ptr(),
                    temporary_wide.as_ptr(),
                    backup_wide.as_ptr(),
                    0,
                    null(),
                    null(),
                )
            } else {
                // No REPLACE_EXISTING: a concurrently created target wins.
                MoveFileExW(
                    temporary_wide.as_ptr(),
                    target_wide.as_ptr(),
                    MOVEFILE_WRITE_THROUGH,
                )
            }
        };
        if published == 0 {
            let error = std::io::Error::last_os_error();
            // ReplaceFile may have renamed the original before a later failure.
            // Restore its backup without overwriting a new external target.
            if backup.exists()
                && unsafe {
                    // SAFETY: the same live paths are used without replacement.
                    MoveFileExW(
                        backup_wide.as_ptr(),
                        target_wide.as_ptr(),
                        MOVEFILE_WRITE_THROUGH,
                    )
                } == 0
            {
                return Err(error).with_context(|| {
                    format!("原生配置替换失败；原文件备份已保留：{}", backup.display())
                });
            }
            return Err(error.into());
        }
        if existed {
            let _ = std::fs::remove_file(&backup);
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(not(any(unix, windows)))]
fn atomic_write(path: &Path, bytes: &[u8], unchanged: impl FnOnce() -> Result<()>) -> Result<()> {
    unchanged()?;
    crate::platform::private_storage::atomic_write_private_bytes(path, bytes)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::os::windows::{ffi::OsStrExt, fs::MetadataExt, fs::OpenOptionsExt};
    use windows_sys::Win32::{
        Security::{
            DACL_SECURITY_INFORMATION, GetFileSecurityW, GetSecurityDescriptorControl,
            GetSecurityDescriptorDacl, SE_DACL_PROTECTED,
        },
        Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE},
    };

    fn dacl(path: &Path) -> (Vec<u8>, bool) {
        let path = path
            .as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>();
        let mut needed = 0;
        // SAFETY: the first call only returns the required buffer size.
        unsafe {
            GetFileSecurityW(
                path.as_ptr(),
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                0,
                &mut needed,
            );
        }
        assert!(needed > 0);
        let mut descriptor = vec![0u32; (needed as usize).div_ceil(4)];
        let mut control = 0;
        let mut revision = 0;
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = std::ptr::null_mut();
        // SAFETY: the DWORD-aligned descriptor buffer covers the returned size;
        // both APIs borrow it only for the duration of their calls.
        unsafe {
            assert_ne!(
                GetFileSecurityW(
                    path.as_ptr(),
                    DACL_SECURITY_INFORMATION,
                    descriptor.as_mut_ptr().cast(),
                    (descriptor.len() * 4) as u32,
                    &mut needed,
                ),
                0
            );
            assert_ne!(
                GetSecurityDescriptorControl(
                    descriptor.as_mut_ptr().cast(),
                    &mut control,
                    &mut revision,
                ),
                0
            );
            assert_ne!(
                GetSecurityDescriptorDacl(
                    descriptor.as_mut_ptr().cast(),
                    &mut present,
                    &mut acl,
                    &mut defaulted,
                ),
                0
            );
            assert_ne!(present, 0);
            assert!(!acl.is_null());
            (
                std::slice::from_raw_parts(acl.cast(), (*acl).AclSize as usize).to_vec(),
                control & SE_DACL_PROTECTED != 0,
            )
        }
    }

    fn no_staging_files(parent: &Path) {
        assert!(!std::fs::read_dir(parent).unwrap().any(|entry| {
            matches!(
                entry
                    .unwrap()
                    .path()
                    .extension()
                    .and_then(|ext| ext.to_str()),
                Some("tmp" | "bak")
            )
        }));
    }

    // This lower-level Windows owner covers native publication/ACL semantics;
    // the existing native-edit owner covers connection merging and rollback.
    #[test]
    fn windows_native_edits_preserve_acl_and_guard_publication() {
        let root = std::env::temp_dir().join(format!("rovai-native-file-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        for protected in [false, true] {
            let parent = root.join(if protected { "private" } else { "ordinary" });
            let path = parent.join("原生配置.json");
            if protected {
                crate::platform::prepare_private_directory(&parent).unwrap();
                crate::platform::atomic_write_private_bytes(&path, b"before").unwrap();
            } else {
                std::fs::create_dir(&parent).unwrap();
                std::fs::write(&path, b"before").unwrap();
                assert!(crate::platform::prepare_private_directory(&parent).is_err());
            }
            let parent_acl = dacl(&parent);
            let original_acl = dacl(&path);
            assert_eq!(original_acl.1, protected);
            let creation_time = std::fs::metadata(&path).unwrap().creation_time();
            let file = NativeFile::read(&path).unwrap();
            file.write(b"after").unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), b"after");
            assert_eq!(dacl(&path), original_acl);
            assert_eq!(
                std::fs::metadata(&path).unwrap().creation_time(),
                creation_time
            );
            file.restore(&Some(b"after".to_vec())).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), b"before");
            assert_eq!(dacl(&path), original_acl);

            atomic_write(&path, b"staged", || {
                let staged = std::fs::read_dir(&parent)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|path| path.extension().is_some_and(|ext| ext == "tmp"))
                    .unwrap();
                assert_eq!(std::fs::read(&staged).unwrap(), b"staged");
                assert!(!dacl(&staged).1);
                file.unchanged()
            })
            .unwrap();
            let file = NativeFile::read(&path).unwrap();
            assert!(
                atomic_write(&path, b"must-not-write", || {
                    std::fs::write(&path, b"external").unwrap();
                    file.unchanged()
                })
                .is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), b"external");
            assert!(file.restore(&Some(b"staged".to_vec())).is_err());

            let permissions = std::fs::metadata(&path).unwrap().permissions();
            let mut readonly = permissions.clone();
            readonly.set_readonly(true);
            std::fs::set_permissions(&path, readonly).unwrap();
            assert!(
                NativeFile::read(&path)
                    .unwrap()
                    .write(b"must-not-write")
                    .unwrap_err()
                    .to_string()
                    .contains("只读")
            );
            std::fs::set_permissions(&path, permissions).unwrap();
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .open(&path)
                .unwrap();
            assert!(
                NativeFile::read(&path)
                    .unwrap()
                    .write(b"must-not-write")
                    .is_err()
            );
            drop(lock);
            assert_eq!(std::fs::read(&path).unwrap(), b"external");
            assert_eq!(dacl(&path), original_acl);
            no_staging_files(&parent);

            let missing = parent.join("new.json");
            let file = NativeFile::read(&missing).unwrap();
            file.write(b"new").unwrap();
            let control = parent.join("control.json");
            std::fs::write(&control, b"ordinary").unwrap();
            assert_eq!(std::fs::read(&missing).unwrap(), b"new");
            assert_eq!(dacl(&missing), dacl(&control));
            assert!(!dacl(&missing).1);
            file.restore(&Some(b"new".to_vec())).unwrap();
            assert!(!missing.exists());
            assert!(
                atomic_write(&missing, b"must-not-write", || {
                    file.unchanged()?;
                    std::fs::write(&missing, b"concurrent creator").unwrap();
                    Ok(())
                })
                .is_err()
            );
            assert_eq!(std::fs::read(&missing).unwrap(), b"concurrent creator");
            assert_eq!(dacl(&parent), parent_acl);
            no_staging_files(&parent);
        }
        #[cfg(feature = "extended-tests")]
        {
            // Junction creation needs no symlink privilege. Keep this process
            // fixture in the extended route while exercising real linked paths.
            let first = root.join("first-target");
            let second = root.join("second-target");
            let linked = root.join("linked-config");
            std::fs::create_dir(&first).unwrap();
            std::fs::create_dir(&second).unwrap();
            std::fs::write(first.join("config.json"), b"before").unwrap();
            std::fs::write(second.join("config.json"), b"before").unwrap();
            let junction = |target: &Path| {
                assert!(
                    std::process::Command::new("cmd.exe")
                        .args(["/D", "/C", "mklink", "/J"])
                        .arg(&linked)
                        .arg(target)
                        .output()
                        .unwrap()
                        .status
                        .success()
                );
            };
            junction(&first);
            let file = NativeFile::read(&linked.join("config.json")).unwrap();
            file.write(b"after").unwrap();
            assert_eq!(std::fs::read(first.join("config.json")).unwrap(), b"after");
            assert!(linked.symlink_metadata().unwrap().file_type().is_symlink());
            std::fs::remove_dir(&linked).unwrap();
            junction(&second);
            assert!(
                file.write(b"must-not-write")
                    .unwrap_err()
                    .to_string()
                    .contains("符号链接目标已变化")
            );
            assert_eq!(
                std::fs::read(second.join("config.json")).unwrap(),
                b"before"
            );
            file.restore(&Some(b"after".to_vec())).unwrap();
            assert_eq!(std::fs::read(first.join("config.json")).unwrap(), b"before");
            let missing = NativeFile::read(&linked.join("missing.json")).unwrap();
            missing.write(b"new").unwrap();
            assert_eq!(std::fs::read(second.join("missing.json")).unwrap(), b"new");
            assert!(linked.symlink_metadata().unwrap().file_type().is_symlink());
            std::fs::remove_dir(&linked).unwrap();
            no_staging_files(&first);
            no_staging_files(&second);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
