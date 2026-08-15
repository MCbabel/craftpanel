use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};

use super::message::Kind;
use super::render::sample;

#[derive(Debug, Subcommand)]
pub enum MailCommand {
    /// Write every mail as HTML and as text with example values.
    Preview(Preview),
}

#[derive(Debug, Args)]
pub struct Preview {
    /// Where the files go. The directory is made if it is not there.
    #[arg(long, default_value = "/tmp/craftpanel-mail")]
    pub out: PathBuf,
    /// One of the eight names; without it all eight are written.
    #[arg(long)]
    pub kind: Option<String>,
}

pub async fn run(command: MailCommand) -> Result<()> {
    match command {
        MailCommand::Preview(args) => {
            let only = match args.kind.as_deref() {
                None => None,
                Some(text) => Some(text.parse::<Kind>().map_err(|_| {
                    anyhow::anyhow!(
                        "{text} is not one of the mails. Pick one of: {}",
                        Kind::ALL.iter().copied().map(Kind::as_str).collect::<Vec<_>>().join(", ")
                    )
                })?),
            };

            for path in write(&args.out, only)? {
                println!("{}", path.display());
            }
            Ok(())
        }
    }
}

pub fn write(out: &Path, only: Option<Kind>) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(out)
        .with_context(|| format!("making the directory {}", out.display()))?;

    let mut written = Vec::new();
    for kind in Kind::ALL.iter().filter(|kind| only.is_none_or(|wanted| wanted == **kind)) {
        let mail = sample(*kind);
        for (suffix, body) in [("html", &mail.html), ("txt", &mail.text)] {
            let path = out.join(format!("{kind}.{suffix}"));
            std::fs::write(&path, body)
                .with_context(|| format!("writing {}", path.display()))?;
            written.push(path);
        }
    }

    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Id;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_preview_writes_two_files_for_every_one_of_the_eight_mails() {
        let scratch = Scratch(std::env::temp_dir().join(format!("craftpanel-preview-{}", Id::new())));

        let written = write(&scratch.0, None).expect("writing the previews");
        assert_eq!(written.len(), 16, "{written:?}");

        for path in &written {
            let body = std::fs::read_to_string(path).expect("a written file");
            assert!(!body.contains("{{"), "{} still has a placeholder", path.display());
            if path.extension().is_some_and(|ext| ext == "html") {
                assert!(body.contains("<title>"), "{} has no title", path.display());
                assert!(body.contains("craftpanel"), "{} has no wordmark", path.display());
            }
        }

        assert!(scratch.0.join("verify_email.html").exists());
        assert!(scratch.0.join("test.txt").exists());
    }

    #[test]
    fn one_mail_can_be_asked_for_on_its_own() {
        let scratch = Scratch(std::env::temp_dir().join(format!("craftpanel-preview-{}", Id::new())));
        let written = write(&scratch.0, Some(Kind::ResetPassword)).expect("writing one preview");

        assert_eq!(written.len(), 2);
        assert!(scratch.0.join("reset_password.html").exists());
        assert!(!scratch.0.join("verify_email.html").exists());
    }
}
