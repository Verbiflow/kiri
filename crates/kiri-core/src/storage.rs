use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn discover() -> Result<Self> {
        let root = if let Some(path) = std::env::var_os("KIRI_CONFIG_DIR") {
            PathBuf::from(path)
        } else if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
            PathBuf::from(path).join("kiri")
        } else {
            PathBuf::from(std::env::var_os("HOME").context("Set HOME or KIRI_CONFIG_DIR")?)
                .join(".config/kiri")
        };
        Ok(Self::at(root))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    pub fn load<T: DeserializeOwned + Default>(&self, name: &str) -> Result<T> {
        read_json(&self.path(name))
    }

    pub fn update<T: DeserializeOwned + Serialize + Default, R>(
        &self,
        name: &str,
        update: impl FnOnce(&mut T) -> Result<R>,
    ) -> Result<R> {
        self.prepare()?;
        let _lock = FileLock::acquire(self.path(&format!("{name}.lock")))?;
        let mut value = self.load(name)?;
        let result = update(&mut value)?;
        atomic_write(&self.path(name), &serde_json::to_vec_pretty(&value)?)?;
        Ok(result)
    }

    pub fn prepare(&self) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }
}

pub struct FileLock {
    _file: File,
}

impl FileLock {
    pub fn acquire(path: impl AsRef<Path>) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        file.try_lock_exclusive()
            .context("Another Kiri operation is using this state. Try again when it finishes.")?;
        Ok(Self { _file: file })
    }
}

pub fn read_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        bail!("State file is too large: {}", path.display());
    }
    serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "Invalid state file: {}. Fix it rather than overwriting it.",
            path.display()
        )
    })
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("State path has no parent")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}
