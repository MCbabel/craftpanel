use std::io::BufRead;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use sqlx::SqlitePool;

use super::{password, reset, session, settings, users};
use crate::config::Config;
use crate::helper::Helper;
use crate::model::{AccountOrigin, Id, PanelRole, PortRange, SystemUserState, Timestamp};

#[derive(Debug, Subcommand)]
pub enum AdminCommand {
    /// Add a panel administrator. This is the way to the *first* one: people can
    /// sign themselves up (section 20), but that path hands out `user` and nothing
    /// else, so a panel with no administrator still needs this.
    Create(Create),
    /// Set somebody's password from the terminal. Throws every session and every
    /// open reset link away (21.8, 21.9).
    Passwd(Passwd),
    /// Put an email address on an account, change it or take it away. Without an
    /// address an account has no "forgot my password" at all (21.7), and the two
    /// accounts an operator makes by hand are exactly the ones that start without
    /// one.
    Email(Email),
    /// Print a reset link for somebody. The one way that works with no Resend key
    /// and no interface — the operator mails it to himself (21.9).
    ResetLink(ResetLink),
    /// Set the range the panel hands server ports out from. The installer asks for
    /// it on a fresh database; afterwards it is the panel's own setting, under
    /// Administration → Settings → Port pool.
    Ports(Ports),
}

#[derive(Debug, Args)]
pub struct Create {
    #[arg(long)]
    pub username: String,
    /// An email address for the account, as 12.3 takes one. Optional here for the
    /// same reason it is optional there — the panel works without mail — but an
    /// account that has one can recover itself later without anybody's help.
    #[arg(long)]
    pub email: Option<String>,
    /// Make one up and write it to standard output.
    #[arg(long)]
    pub print_password: bool,
    /// Read one from the first line of standard input instead.
    #[arg(long, conflicts_with = "print_password")]
    pub password_stdin: bool,
}

#[derive(Debug, Args)]
pub struct Passwd {
    #[arg(long)]
    pub username: String,
    #[arg(long)]
    pub print_password: bool,
    #[arg(long, conflicts_with = "print_password")]
    pub password_stdin: bool,
}

#[derive(Debug, Args)]
pub struct Email {
    #[arg(long)]
    pub username: String,
    /// Trimmed and folded to lower case, as everywhere else (20.10). Refused when
    /// another account or an open sign-up already holds it.
    #[arg(long, conflicts_with = "remove")]
    pub address: Option<String>,
    /// Take the address off the account, and with it its way back over mail.
    #[arg(long)]
    pub remove: bool,
}

#[derive(Debug, Args)]
pub struct Ports {
    #[arg(long)]
    pub from: u16,
    #[arg(long)]
    pub to: u16,
}

#[derive(Debug, Args)]
pub struct ResetLink {
    #[arg(long)]
    pub username: String,
    /// Where the panel is reached, with the scheme. Needed when `link_base` (19.2)
    /// is not set yet — which is the state on the first day.
    #[arg(long)]
    pub base_url: Option<String>,
    #[arg(long)]
    pub minutes: Option<i64>,
}

pub async fn run(command: AdminCommand) -> Result<()> {
    match command {
        AdminCommand::Create(args) => create(args).await,
        AdminCommand::Passwd(args) => passwd(args).await,
        AdminCommand::Email(args) => email(args).await,
        AdminCommand::ResetLink(args) => reset_link(args).await,
        AdminCommand::Ports(args) => ports(args).await,
    }
}

async fn create(args: Create) -> Result<()> {
    let config = Config::load(&config_path())?;
    let pool = crate::db::connect(&config.database_path()).await?;

    let secret = match (args.print_password, args.password_stdin) {
        (true, _) => made_up_password(),
        (_, true) => read_password()?,
        _ => bail!("pass --print-password to have one made up, or --password-stdin to give one"),
    };

    let helper = Helper::new(&config.helper_socket);
    let id = install(
        &pool,
        &helper,
        &args.username,
        args.email.as_deref(),
        &secret,
        args.print_password,
    )
    .await?;

    if args.print_password {
        println!("{secret}");
    } else {
        eprintln!("created {} ({id})", args.username);
    }
    Ok(())
}

async fn passwd(args: Passwd) -> Result<()> {
    let config = Config::load(&config_path())?;
    let pool = crate::db::connect(&config.database_path()).await?;

    let secret = match (args.print_password, args.password_stdin) {
        (true, _) => made_up_password(),
        (_, true) => read_password()?,
        _ => bail!("pass --print-password to have one made up, or --password-stdin to give one"),
    };

    let (id, closed, links) = set_password(&pool, &args.username, &secret, args.print_password)
        .await?;

    if args.print_password {
        println!("{secret}");
    }
    eprintln!("set the password of {} ({id})", args.username);
    eprintln!("closed {closed} session(s) and threw away {links} open reset link(s)");
    Ok(())
}

async fn set_password(
    pool: &SqlitePool,
    username: &str,
    secret: &str,
    must_change_password: bool,
) -> Result<(Id, u64, u64)> {
    let row = users::by_name(pool, username)
        .await?
        .ok_or_else(|| anyhow::anyhow!("{username} has no account"))?;

    let hash = password::hash(secret).map_err(|refusal| anyhow::anyhow!("{refusal}"))?;
    sqlx::query(
        "UPDATE users SET password_hash = ?, must_change_password = ?, updated_at = ? WHERE id = ?",
    )
    .bind(hash)
    .bind(must_change_password)
    .bind(Timestamp::now())
    .bind(row.id)
    .execute(pool)
    .await
    .context("writing the password")?;

    let closed = session::close_all_of(pool, row.id, None).await?;
    let links = reset::forget_all(pool, row.id).await?;
    Ok((row.id, closed, links))
}

async fn email(args: Email) -> Result<()> {
    let config = Config::load(&config_path())?;
    let pool = crate::db::connect(&config.database_path()).await?;

    let wanted = match (args.address.as_deref(), args.remove) {
        (Some(typed), _) => Some(typed),
        (None, true) => None,
        _ => bail!("pass --address max@example.test, or --remove to take the address away"),
    };

    let (id, before, links) = set_email(&pool, &args.username, wanted).await?;
    let after = users::load(&pool, id).await?.email;

    match (&before, &after) {
        (_, Some(address)) => eprintln!("{} now reaches {address}", args.username),
        (Some(old), None) => eprintln!("{} no longer has an address; {old} is gone", args.username),
        (None, None) => eprintln!("{} had no address anyway", args.username),
    }
    if links > 0 {
        eprintln!("threw away {links} open reset link(s), because they went to the old address");
    }
    if after.is_none() {
        eprintln!(
            "note: without an address this account cannot recover itself — `admin passwd` and \
             `admin reset-link` are what is left for it"
        );
    }
    Ok(())
}

async fn set_email(
    pool: &SqlitePool,
    username: &str,
    wanted: Option<&str>,
) -> Result<(Id, Option<String>, u64)> {
    let row = users::by_name(pool, username)
        .await?
        .ok_or_else(|| anyhow::anyhow!("{username} has no account"))?;

    let address = match wanted {
        Some(typed) => Some(normalised(pool, typed, Some(row.id)).await?),
        None => None,
    };
    if address == row.email {
        return Ok((row.id, row.email, 0));
    }

    sqlx::query("UPDATE users SET email = ?, updated_at = ? WHERE id = ?")
        .bind(address.as_deref())
        .bind(Timestamp::now())
        .bind(row.id)
        .execute(pool)
        .await
        .map_err(|err| anyhow::anyhow!("{}", users::map_taken(err)))
        .context("writing the address")?;

    let links = reset::forget_all(pool, row.id).await?;
    Ok((row.id, row.email, links))
}

async fn normalised(pool: &SqlitePool, typed: &str, except: Option<Id>) -> Result<String> {
    let address = crate::registration::address::normalise(typed)
        .map_err(|refusal| anyhow::anyhow!("{refusal}"))?;
    users::claim_email(pool, &address, except)
        .await
        .map_err(|refusal| anyhow::anyhow!("{refusal}"))?;
    Ok(address)
}

async fn reset_link(args: ResetLink) -> Result<()> {
    let config = Config::load(&config_path())?;
    let pool = crate::db::connect(&config.database_path()).await?;

    let row = users::by_name(&pool, &args.username)
        .await?
        .ok_or_else(|| anyhow::anyhow!("{} has no account", args.username))?;

    let base = match args.base_url {
        Some(given) => given,
        None => crate::mail::store::load(&pool).await?.link_base.ok_or_else(|| {
            anyhow::anyhow!(
                "no panel address is set yet; pass --base-url https://panel.example.com"
            )
        })?,
    };
    let base = base.trim_end_matches('/').to_owned();
    if !base.starts_with("http://") && !base.starts_with("https://") {
        bail!("--base-url needs a scheme, for example https://panel.example.com");
    }
    if base.starts_with("http://") {
        eprintln!("warning: the link carries a token and http:// sends it in the clear");
    }

    let life = args.minutes.map(time::Duration::minutes);
    if life.is_some_and(|life| life <= time::Duration::ZERO) {
        bail!("--minutes has to be above zero");
    }

    reset::forget_all(&pool, row.id).await?;
    let token = reset::mint_for(&pool, row.id, life, None, Timestamp::now()).await?;

    println!("{base}/reset-password#{token}");
    eprintln!(
        "valid for {} minutes, once. Nothing was mailed — send it yourself.",
        life.unwrap_or(reset::LIFETIME).whole_minutes()
    );
    Ok(())
}

async fn ports(args: Ports) -> Result<()> {
    let config = Config::load(&config_path())?;
    let pool = crate::db::connect(&config.database_path()).await?;

    let wanted = PortRange { from: args.from, to: args.to };
    let moved = set_ports(&pool, wanted).await?;

    if moved {
        eprintln!("servers take their ports from {} to {} now", wanted.from, wanted.to);
    } else {
        eprintln!("servers already took their ports from {} to {}", wanted.from, wanted.to);
    }
    Ok(())
}

pub async fn set_ports(pool: &SqlitePool, wanted: PortRange) -> Result<bool> {
    let mut current = settings::load(pool).await?;
    if current.port_pool == wanted {
        return Ok(false);
    }

    current.port_pool = wanted;
    settings::save(pool, &current).await.map_err(|refusal| anyhow::anyhow!("{refusal}"))?;
    Ok(true)
}

pub fn config_path() -> PathBuf {
    std::env::var_os("CRAFTPANEL_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/craftpanel/config.toml"))
}

async fn install(
    pool: &SqlitePool,
    helper: &Helper,
    username: &str,
    address: Option<&str>,
    secret: &str,
    must_change_password: bool,
) -> Result<Id> {
    users::check_username(username).map_err(|refusal| anyhow::anyhow!("{refusal}"))?;
    if users::by_name(pool, username).await?.is_some() {
        bail!("{username} already has an account");
    }

    let email = match address {
        Some(typed) => Some(normalised(pool, typed, None).await?),
        None => None,
    };

    let row = users::insert(
        pool,
        users::NewUser {
            username,
            email,
            origin: AccountOrigin::Admin,
            password_hash: password::hash(secret)
                .map_err(|refusal| anyhow::anyhow!("{refusal}"))?,
            role: PanelRole::Admin,
            must_change_password,
            limits: settings::load(pool).await?.default_limits,
        },
    )
    .await
    .context("writing the administrator")?;

    let system = users::provision(pool, helper, &row).await?;
    if system.state == SystemUserState::Error {
        sqlx::query("UPDATE users SET system_state = 'provisioning' WHERE id = ?")
            .bind(row.id)
            .execute(pool)
            .await?;
        eprintln!(
            "note: no system account yet ({}); the panel will see to it when it starts",
            system.error_message.unwrap_or_default()
        );
    }

    Ok(row.id)
}

const ALPHABET: &[u8; 32] = b"abcdefghjkmnpqrstvwxyz23456789_-";

fn made_up_password() -> String {
    rand::random::<[u8; 20]>().iter().map(|byte| ALPHABET[*byte as usize % 32] as char).collect()
}

fn read_password() -> Result<String> {
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).context("reading the password")?;
    let secret = line.trim_end_matches(['\r', '\n']).to_owned();
    if secret.is_empty() {
        bail!("no password on standard input");
    }
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::harness::{a_server, a_user, test_pool, FakeHelper};
    use crate::auth::users;
    use crate::model::SystemUserState;

    #[tokio::test]
    async fn the_installer_moves_the_range_the_panel_hands_ports_out_from() {
        let pool = test_pool().await;
        assert_eq!(
            settings::load(&pool).await.unwrap().port_pool,
            PortRange { from: 25565, to: 25700 },
            "0002 writes this one, and nothing in config.toml ever changed it"
        );

        let wanted = PortRange { from: 25800, to: 25850 };
        assert!(set_ports(&pool, wanted).await.unwrap(), "the range moved");
        assert_eq!(settings::load(&pool).await.unwrap().port_pool, wanted);

        assert!(!set_ports(&pool, wanted).await.unwrap(), "asked for the range it already had");
    }

    #[tokio::test]
    async fn a_range_that_would_strand_a_server_is_refused() {
        let pool = test_pool().await;
        let max = a_user(&pool, "max").await;
        let server = a_server(&pool, max, "one", 2048).await;
        sqlx::query(
            "INSERT INTO allocations (port, server_id, name, is_primary, created_at) \
             VALUES (25565, ?, 'game', 1, ?)",
        )
        .bind(server)
        .bind(Timestamp::now())
        .execute(&pool)
        .await
        .unwrap();

        let refusal =
            set_ports(&pool, PortRange { from: 25800, to: 25850 }).await.unwrap_err().to_string();
        assert!(refusal.contains("25565"), "{refusal}");
        assert_eq!(
            settings::load(&pool).await.unwrap().port_pool,
            PortRange { from: 25565, to: 25700 },
            "the refusal left the setting alone"
        );
    }

    #[test]
    fn a_made_up_password_is_long_and_unmistakable() {
        let secret = made_up_password();
        assert_eq!(secret.chars().count(), 20);
        assert!(secret.chars().all(|c| ALPHABET.contains(&(c as u8))), "{secret}");
        assert!(!secret.contains(['i', 'l', 'o', 'u']), "the four that get misread: {secret}");
        assert!(password::check_strength(&secret).is_ok());
        assert_ne!(secret, made_up_password());
    }

    #[tokio::test]
    async fn the_first_admin_can_sign_in_and_manage_users() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;

        let id = install(&pool, &Helper::new(fake.socket()), "admin", None, "a-good-password", true)
            .await
            .unwrap();

        let row = users::load(&pool, id).await.unwrap();
        assert_eq!(row.username, "admin");
        assert!(row.is_admin());
        assert!(row.must_change_password, "a password out of a terminal wants replacing");
        assert!(password::verify("a-good-password", &row.password_hash));
        assert_eq!(row.system_state, SystemUserState::Ready);
        assert_eq!(row.memory_mib, 4096, "the panel default limits");
    }

    #[tokio::test]
    async fn without_a_helper_the_account_waits_instead_of_failing() {
        let pool = test_pool().await;
        let nowhere = Helper::new("/run/craftpanel/there-is-no-helper.sock");

        let id = install(&pool, &nowhere, "admin", None, "a-good-password", true).await.unwrap();

        let row = users::load(&pool, id).await.unwrap();
        assert_eq!(
            row.system_state,
            SystemUserState::Provisioning,
            "the installer starts the helper after this call"
        );
        assert!(row.system_error_message.is_some(), "and it says why");

        let fake = FakeHelper::obliging().await;
        assert_eq!(users::reconcile(&pool, &Helper::new(fake.socket())).await.unwrap(), 1);
        assert_eq!(users::load(&pool, id).await.unwrap().system_state, SystemUserState::Ready);
    }

    #[tokio::test]
    async fn a_name_is_taken_only_once() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let helper = Helper::new(fake.socket());

        install(&pool, &helper, "admin", None, "a-good-password", true).await.unwrap();
        let second = install(&pool, &helper, "admin", None, "a-good-password", true).await;
        assert!(second.unwrap_err().to_string().contains("already has an account"));
    }

    #[tokio::test]
    async fn the_terminal_can_set_a_password_and_throws_the_old_ways_out() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        install(&pool, &Helper::new(fake.socket()), "admin", None, "first-password", true).await.unwrap();

        let row = users::by_name(&pool, "admin").await.unwrap().unwrap();
        crate::auth::session::open(&pool, row.id, None, Timestamp::now()).await.unwrap();
        crate::auth::session::open(&pool, row.id, None, Timestamp::now()).await.unwrap();
        reset::mint_for(&pool, row.id, None, None, Timestamp::now()).await.unwrap();

        let (id, closed, links) =
            set_password(&pool, "admin", "second-password", false).await.unwrap();

        assert_eq!(id, row.id);
        assert_eq!(closed, 2, "every session of that account");
        assert_eq!(links, 1, "and every open reset link (21.8)");

        let after = users::load(&pool, row.id).await.unwrap();
        assert!(password::verify("second-password", &after.password_hash));
        assert!(!password::verify("first-password", &after.password_hash), "the old one is gone");
        assert!(
            !after.must_change_password,
            "he typed it himself, so it does not want replacing"
        );
    }

    #[tokio::test]
    async fn a_password_out_of_the_terminal_wants_replacing_but_one_he_typed_does_not() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        install(&pool, &Helper::new(fake.socket()), "admin", None, "first-password", false).await.unwrap();

        set_password(&pool, "admin", &made_up_password(), true).await.unwrap();
        assert!(users::by_name(&pool, "admin").await.unwrap().unwrap().must_change_password);

        set_password(&pool, "admin", "self-chosen-one", false).await.unwrap();
        assert!(!users::by_name(&pool, "admin").await.unwrap().unwrap().must_change_password);
    }

    #[tokio::test]
    async fn the_terminal_refuses_what_the_endpoint_refuses() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        install(&pool, &Helper::new(fake.socket()), "admin", None, "first-password", true).await.unwrap();

        let unknown = set_password(&pool, "nobody", "a-good-password", false).await;
        assert!(unknown.unwrap_err().to_string().contains("no account"));

        let weak = set_password(&pool, "admin", "short", false).await;
        assert_eq!(weak.unwrap_err().to_string(), "weak_password: a password needs at least 10 characters");

        let row = users::by_name(&pool, "admin").await.unwrap().unwrap();
        assert!(password::verify("first-password", &row.password_hash));
    }

    #[tokio::test]
    async fn the_same_rules_as_the_endpoint_apply_to_the_terminal() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let helper = Helper::new(fake.socket());

        assert!(install(&pool, &helper, "Admin", None, "a-good-password", true).await.is_err());
        assert!(install(&pool, &helper, "ad", None, "a-good-password", true).await.is_err());
        assert!(install(&pool, &helper, "admin", None, "short", true).await.is_err());

        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM users").fetch_one(&pool).await.unwrap();
        assert_eq!(left, 0, "a refused account leaves no row");
    }

    #[tokio::test]
    async fn the_terminal_can_set_change_and_take_away_an_address() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        install(&pool, &Helper::new(fake.socket()), "admin", None, "first-password", true)
            .await
            .unwrap();

        let (id, before, links) = set_email(&pool, "admin", Some("  Chef@Example.TEST \n")).await.unwrap();
        assert_eq!(before, None);
        assert_eq!(links, 0, "there was no link to throw away");
        assert_eq!(users::load(&pool, id).await.unwrap().email.as_deref(), Some("chef@example.test"));

        let (_, before, _) = set_email(&pool, "admin", Some("new@example.test")).await.unwrap();
        assert_eq!(before.as_deref(), Some("chef@example.test"));

        let (_, before, _) = set_email(&pool, "admin", None).await.unwrap();
        assert_eq!(before.as_deref(), Some("new@example.test"));
        assert_eq!(users::load(&pool, id).await.unwrap().email, None);
    }

    #[tokio::test]
    async fn a_changed_address_throws_the_open_links_away() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        install(&pool, &Helper::new(fake.socket()), "admin", Some("old@example.test"), "first-password", true)
            .await
            .unwrap();
        let row = users::by_name(&pool, "admin").await.unwrap().unwrap();
        reset::mint_for(&pool, row.id, None, None, Timestamp::now()).await.unwrap();

        let (_, _, links) = set_email(&pool, "admin", Some("new@example.test")).await.unwrap();
        assert_eq!(links, 1);

        reset::mint_for(&pool, row.id, None, None, Timestamp::now()).await.unwrap();
        let (_, _, links) = set_email(&pool, "admin", Some("NEW@example.test")).await.unwrap();
        assert_eq!(links, 0, "the same address, only spelled differently");
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM password_resets")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 1);
    }

    #[tokio::test]
    async fn the_terminal_refuses_an_address_that_is_taken_or_is_no_address() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let helper = Helper::new(fake.socket());
        install(&pool, &helper, "admin", Some("chef@example.test"), "first-password", true)
            .await
            .unwrap();

        crate::registration::store::insert(
            &pool,
            crate::registration::store::NewApplication {
                username: "max",
                email: "max@example.test",
                password_hash: "x".to_owned(),
                signup_ip: None,
                token_hash: crate::auth::secret::digest("something"),
                token_expires_at: Timestamp::now(),
            },
            Timestamp::now(),
        )
        .await
        .unwrap();

        let taken = set_email(&pool, "admin", Some("max@example.test")).await;
        assert!(taken.unwrap_err().to_string().contains("email_taken"));

        let nonsense = set_email(&pool, "admin", Some("no-at-sign")).await;
        assert!(nonsense.unwrap_err().to_string().contains("invalid_email"));

        let nobody = set_email(&pool, "nobody", Some("who@example.test")).await;
        assert!(nobody.unwrap_err().to_string().contains("no account"));

        let row = users::by_name(&pool, "admin").await.unwrap().unwrap();
        assert_eq!(row.email.as_deref(), Some("chef@example.test"));
    }

    #[tokio::test]
    async fn the_first_administrator_may_bring_an_address_along() {
        let pool = test_pool().await;
        let fake = FakeHelper::obliging().await;
        let helper = Helper::new(fake.socket());

        let id = install(&pool, &helper, "admin", Some(" Chef@Example.test "), "a-good-password", true)
            .await
            .unwrap();
        assert_eq!(users::load(&pool, id).await.unwrap().email.as_deref(), Some("chef@example.test"));

        let again = install(&pool, &helper, "second", Some("chef@example.test"), "a-good-password", true).await;
        assert!(again.unwrap_err().to_string().contains("email_taken"));
        assert!(users::by_name(&pool, "second").await.unwrap().is_none(), "a refused account leaves no row");
    }
}
