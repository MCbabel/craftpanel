use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;

use crate::model::Id;

use super::oauth::Secret;

#[derive(Clone)]
pub struct Keys {
    dir: PathBuf,
}

impl Keys {
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn client_secret_path(&self) -> PathBuf {
        self.dir.join("client_secret")
    }

    pub fn user_dir(&self, user: Id) -> PathBuf {
        self.dir.join(user.to_string())
    }

    pub fn refresh_token_path(&self, user: Id) -> PathBuf {
        self.user_dir(user).join("refresh_token")
    }

    pub async fn read_client_secret(&self) -> Option<Secret> {
        read(&self.client_secret_path()).await
    }

    pub async fn write_client_secret(&self, secret: &Secret) -> io::Result<()> {
        write(&self.dir, &self.client_secret_path(), secret).await
    }

    pub async fn forget_client_secret(&self) -> io::Result<()> {
        forget(&self.client_secret_path()).await
    }

    pub async fn read_refresh_token(&self, user: Id) -> Option<Secret> {
        read(&self.refresh_token_path(user)).await
    }

    pub async fn write_refresh_token(&self, user: Id, secret: &Secret) -> io::Result<()> {
        write(&self.user_dir(user), &self.refresh_token_path(user), secret).await
    }

    pub async fn forget_refresh_token(&self, user: Id) -> io::Result<()> {
        forget(&self.refresh_token_path(user)).await
    }

    pub async fn token_written_at(&self, user: Id) -> Option<std::time::SystemTime> {
        tokio::fs::metadata(self.refresh_token_path(user)).await.ok()?.modified().ok()
    }

    pub async fn forget_user(&self, user: Id) -> io::Result<()> {
        match tokio::fs::remove_dir_all(self.user_dir(user)).await {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

async fn read(path: &Path) -> Option<Secret> {
    Secret::parse(&tokio::fs::read_to_string(path).await.ok()?)
}

async fn write(dir: &Path, path: &Path, secret: &Secret) -> io::Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    set_mode(dir, 0o700).await?;

    let partial = path.with_extension("part");
    let mut file = tokio::fs::File::create(&partial).await?;
    set_mode(&partial, 0o600).await?;
    file.write_all(secret.expose().as_bytes()).await?;
    file.write_all(b"\n").await?;
    file.sync_all().await?;
    drop(file);

    tokio::fs::rename(&partial, path).await
}

async fn forget(path: &Path) -> io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

async fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("craftpanel-drive-keys-{}", Id::new())))
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
    async fn the_client_secret_lands_as_0600_in_a_0700_directory() {
        let scratch = Scratch::new();
        let keys = Keys::in_dir(&scratch.0);
        assert!(keys.read_client_secret().await.is_none(), "nothing is set up yet");

        let secret = Secret::parse("GOCSPX-pretend-this-is-real").expect("a secret");
        keys.write_client_secret(&secret).await.expect("writing it");

        assert_eq!(mode(&scratch.0), 0o700, "the directory");
        assert_eq!(mode(&keys.client_secret_path()), 0o600, "the file");
        assert_eq!(
            keys.read_client_secret().await.expect("a secret").expose(),
            "GOCSPX-pretend-this-is-real"
        );
        assert!(
            !scratch.0.join("client_secret.part").exists(),
            "the partial file is moved, not left"
        );

        keys.forget_client_secret().await.expect("removing it");
        assert!(keys.read_client_secret().await.is_none());
        keys.forget_client_secret().await.expect("forgetting twice is not an error");
    }

    #[tokio::test]
    async fn two_users_keep_their_own_token_in_their_own_0700_directory() {
        let scratch = Scratch::new();
        let keys = Keys::in_dir(&scratch.0);
        let (anna, ben) = (Id::new(), Id::new());

        keys.write_refresh_token(anna, &Secret::parse("1//anna").expect("a token"))
            .await
            .expect("Anna's token");
        keys.write_refresh_token(ben, &Secret::parse("1//ben").expect("a token"))
            .await
            .expect("Ben's token");

        assert_eq!(keys.read_refresh_token(anna).await.expect("a token").expose(), "1//anna");
        assert_eq!(keys.read_refresh_token(ben).await.expect("a token").expose(), "1//ben");
        assert_eq!(mode(&keys.user_dir(anna)), 0o700);
        assert_eq!(mode(&keys.refresh_token_path(anna)), 0o600);

        keys.forget_refresh_token(anna).await.expect("removing Anna's");
        assert!(keys.read_refresh_token(anna).await.is_none());
        assert!(keys.read_refresh_token(ben).await.is_some(), "Ben's is untouched");

        keys.forget_user(ben).await.expect("removing Ben's directory");
        assert!(!keys.user_dir(ben).exists());
        keys.forget_user(ben).await.expect("twice is not an error");
    }
}
