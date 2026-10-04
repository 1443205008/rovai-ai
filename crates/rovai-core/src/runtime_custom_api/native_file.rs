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
        atomic_write(&self.target, bytes, || self.unchanged())
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

#[cfg(not(unix))]
fn atomic_write(path: &Path, bytes: &[u8], unchanged: impl FnOnce() -> Result<()>) -> Result<()> {
    unchanged()?;
    crate::platform::private_storage::atomic_write_private_bytes(path, bytes)
}
