use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;

use super::resend::ApiKey;

pub struct KeyFile {
    dir: PathBuf,
}

impl KeyFile {
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("api_key")
    }

    pub async fn read(&self) -> Option<ApiKey> {
        let text = tokio::fs::read_to_string(self.path()).await.ok()?;
        ApiKey::parse(&text)
    }

    pub async fn write(&self, key: &ApiKey) -> io::Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        set_mode(&self.dir, 0o700).await?;

        let path = self.path();
        let partial = path.with_extension("part");
        let mut file = tokio::fs::File::create(&partial).await?;
        set_mode(&partial, 0o600).await?;
        file.write_all(key.expose().as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.sync_all().await?;
        drop(file);

        tokio::fs::rename(&partial, &path).await
    }

    pub async fn forget(&self) -> io::Result<()> {
        match tokio::fs::remove_file(self.path()).await {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

async fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Id;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("craftpanel-mail-key-{}", Id::new()));
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).expect("the path exists").permissions().mode() & 0o777
    }

    #[tokio::test]
    async fn the_key_lands_as_0600_in_a_0700_directory_and_comes_back_whole() {
        let scratch = Scratch::new();
        let file = KeyFile::in_dir(&scratch.0);
        assert!(file.read().await.is_none(), "nothing is set up yet");

        let key = ApiKey::parse("re_pretend_this_is_real").expect("a key");
        file.write(&key).await.expect("writing the key");

        assert_eq!(mode(&scratch.0), 0o700, "the directory");
        assert_eq!(mode(&scratch.0.join("api_key")), 0o600, "the file");
        assert_eq!(file.read().await.expect("a key").expose(), "re_pretend_this_is_real");
        assert!(!scratch.0.join("api_key.part").exists(), "the partial file is moved, not left");

        let second = ApiKey::parse("re_another").expect("a key");
        file.write(&second).await.expect("writing again");
        assert_eq!(file.read().await.expect("a key").expose(), "re_another");
    }

    #[tokio::test]
    async fn forgetting_a_key_that_is_not_there_is_not_a_failure() {
        let scratch = Scratch::new();
        let file = KeyFile::in_dir(&scratch.0);
        file.forget().await.expect("no file, no complaint");

        let key = ApiKey::parse("re_x").expect("a key");
        file.write(&key).await.expect("writing the key");
        file.forget().await.expect("removing the key");
        assert!(file.read().await.is_none());
        file.forget().await.expect("still no complaint");
    }
}
