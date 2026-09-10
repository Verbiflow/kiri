use crate::model::RepoPath;
use anyhow::{Context, Result, bail};
use std::{fs::File, path::Path};

pub(crate) enum Entry {
    Missing,
    Link(Vec<u8>),
    File(File),
}
#[cfg(unix)]
pub(crate) fn open(root: &Path, path: &RepoPath) -> Result<Entry> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags, openat, readlinkat, statat};
    let mut directory = rustix::fs::open(root, OFlags::RDONLY | OFlags::DIRECTORY, Mode::empty())?;
    let pathbuf = path.to_path_buf();
    let mut components = pathbuf.components().peekable();
    let name = loop {
        let part = components
            .next()
            .context("Empty repository path")?
            .as_os_str();
        if components.peek().is_none() {
            break part;
        }
        match openat(
            &directory,
            part,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        ) {
            Ok(next) => directory = next,
            Err(rustix::io::Errno::NOENT) => return Ok(Entry::Missing),
            Err(error) => return Err(error.into()),
        }
    };
    let metadata = match statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(metadata) => metadata,
        Err(rustix::io::Errno::NOENT) => return Ok(Entry::Missing),
        Err(error) => return Err(error.into()),
    };
    match FileType::from_raw_mode(metadata.st_mode) {
        FileType::Symlink => Ok(Entry::Link(
            readlinkat(&directory, name, Vec::new())?
                .as_bytes()
                .to_vec(),
        )),
        FileType::RegularFile => Ok(Entry::File(File::from(openat(
            &directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW,
            Mode::empty(),
        )?))),
        _ => bail!("Unsupported working file type: {path}"),
    }
}
#[cfg(not(unix))]
pub(crate) fn open(_root: &Path, _path: &RepoPath) -> Result<Entry> {
    bail!("The platform capability filesystem backend is unavailable")
}
