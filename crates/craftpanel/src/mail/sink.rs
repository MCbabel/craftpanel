use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;

use super::message::Kind;
use super::render::Rendered;
use crate::model::{Id, Timestamp};

pub const VARIABLE: &str = "CRAFTPANEL_MAIL_SINK";

pub struct Sink {
    dir: PathBuf,
}

impl Sink {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os(VARIABLE)?;
        let dir = PathBuf::from(path);
        if dir.as_os_str().is_empty() {
            return None;
        }
        Some(Self { dir })
    }

    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub async fn write(
        &self,
        id: Id,
        kind: Kind,
        when: Timestamp,
        mail: &Rendered,
    ) -> io::Result<String> {
        tokio::fs::create_dir_all(&self.dir).await?;
        set_mode(&self.dir, 0o700).await?;

        let stem = format!("{}-{kind}-{id}", when.to_string().replace(':', "-"));
        for (suffix, body) in [("html", &mail.html), ("txt", &mail.text)] {
            let path = self.dir.join(format!("{stem}.{suffix}"));
            let mut file = tokio::fs::File::create(&path).await?;
            set_mode(&path, 0o600).await?;
            file.write_all(body.as_bytes()).await?;
            file.sync_all().await?;
        }

        Ok(stem)
    }
}

async fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::render::sample;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch() -> Scratch {
        Scratch(std::env::temp_dir().join(format!("craftpanel-mail-sink-{}", Id::new())))
    }

    #[tokio::test]
    async fn a_mail_becomes_two_files_only_its_owner_can_read() {
        let scratch = scratch();
        let sink = Sink::at(&scratch.0);
        let id = Id::new();
        let when: Timestamp = "2026-08-13T21:10:00Z".parse().expect("a moment");

        let stem = sink
            .write(id, Kind::VerifyEmail, when, &sample(Kind::VerifyEmail))
            .await
            .expect("writing the mail");

        assert_eq!(stem, format!("2026-08-13T21-10-00Z-verify_email-{id}"));
        let html = scratch.0.join(format!("{stem}.html"));
        let text = scratch.0.join(format!("{stem}.txt"));

        let mode = |path: &Path| {
            std::fs::metadata(path).expect("the file exists").permissions().mode() & 0o777
        };
        assert_eq!(mode(&scratch.0), 0o700);
        assert_eq!(mode(&html), 0o600);
        assert_eq!(mode(&text), 0o600);

        let written = std::fs::read_to_string(&html).expect("the html");
        assert!(written.contains("Confirm email address"));
        assert!(!written.contains("{{"));
        assert!(std::fs::read_to_string(&text).expect("the text").contains("panel.example"));
    }
}
