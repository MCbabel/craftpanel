use crate::model::{Timestamp, UpdateChannel};

use super::modrinth::MrVersion;

pub fn normalise(version_type: &str) -> UpdateChannel {
    match version_type {
        "alpha" => UpdateChannel::Alpha,
        "beta" => UpdateChannel::Beta,
        _ => UpdateChannel::Release,
    }
}

fn rank(channel: UpdateChannel) -> u8 {
    match channel {
        UpdateChannel::Release => 0,
        UpdateChannel::Beta => 1,
        UpdateChannel::Alpha => 2,
    }
}

pub fn effective(policy: UpdateChannel, installed_type: Option<&str>) -> UpdateChannel {
    let Some(installed) = installed_type else { return policy };
    let installed = normalise(installed);
    if rank(installed) > rank(policy) {
        installed
    } else {
        policy
    }
}

fn fallbacks(policy: UpdateChannel) -> &'static [&'static [UpdateChannel]] {
    use UpdateChannel::*;
    match policy {
        Release => &[&[Release], &[Beta], &[Alpha]],
        Beta => &[&[Release, Beta], &[Alpha]],
        Alpha => &[&[Release, Beta, Alpha]],
    }
}

pub fn allows(version_type: &str, policy: UpdateChannel, installed_type: Option<&str>) -> bool {
    fallbacks(effective(policy, installed_type))[0].contains(&normalise(version_type))
}

pub fn newest_eligible<'a>(
    versions: &'a [MrVersion],
    installed_id: &str,
    installed_published: Option<Timestamp>,
    policy: UpdateChannel,
    installed_type: Option<&str>,
) -> Option<&'a MrVersion> {
    let mut sorted: Vec<&MrVersion> = versions.iter().collect();
    sorted.sort_by(|left, right| right.published().cmp(&left.published()));

    for step in fallbacks(effective(policy, installed_type)) {
        let anything_here =
            versions.iter().any(|version| step.contains(&normalise(&version.version_type)));
        if !anything_here {
            continue;
        }

        return sorted.into_iter().find(|version| {
            if version.id == installed_id {
                return false;
            }
            if !step.contains(&normalise(&version.version_type)) {
                return false;
            }
            match installed_published {
                None => true,
                Some(installed) => version.published().is_some_and(|when| when > installed),
            }
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::modrinth::a_version;

    fn field() -> Vec<MrVersion> {
        vec![
            a_version("release-old", "P", "release", "2026-01-01T00:00:00Z"),
            a_version("release-new", "P", "release", "2026-05-01T00:00:00Z"),
            a_version("beta-newer", "P", "beta", "2026-06-01T00:00:00Z"),
            a_version("alpha-newest", "P", "alpha", "2026-07-01T00:00:00Z"),
        ]
    }

    fn at(text: &str) -> Option<Timestamp> {
        Some(text.parse().expect("a timestamp"))
    }

    #[test]
    fn a_release_server_is_offered_the_newest_release_and_not_the_newer_beta() {
        let field = field();
        let picked = newest_eligible(
            &field,
            "release-old",
            at("2026-01-01T00:00:00Z"),
            UpdateChannel::Release,
            Some("release"),
        );
        assert_eq!(picked.expect("an update").id, "release-new");
    }

    #[test]
    fn a_mod_already_on_a_beta_keeps_getting_betas_on_a_release_server() {
        let field = field();
        let picked = newest_eligible(
            &field,
            "beta-old",
            at("2026-02-01T00:00:00Z"),
            UpdateChannel::Release,
            Some("beta"),
        );
        assert_eq!(
            picked.expect("an update").id,
            "beta-newer",
            "effectiveUpdateChannel raises the policy to the installed channel"
        );
    }

    #[test]
    fn the_installed_version_is_never_offered_back_to_itself() {
        let field = field();
        let picked = newest_eligible(
            &field,
            "release-new",
            at("2026-05-01T00:00:00Z"),
            UpdateChannel::Release,
            Some("release"),
        );
        assert!(picked.is_none(), "the newest release is the installed one");
    }

    #[test]
    fn an_older_version_is_not_an_update() {
        let field = field();
        let picked = newest_eligible(
            &field,
            "somewhere-else",
            at("2030-01-01T00:00:00Z"),
            UpdateChannel::Alpha,
            None,
        );
        assert!(picked.is_none());
    }

    #[test]
    fn the_widening_only_happens_when_the_narrower_channel_is_empty() {
        let only_betas = vec![
            a_version("b1", "P", "beta", "2026-01-01T00:00:00Z"),
            a_version("b2", "P", "beta", "2026-02-01T00:00:00Z"),
        ];
        let picked = newest_eligible(
            &only_betas,
            "b1",
            at("2026-01-01T00:00:00Z"),
            UpdateChannel::Release,
            Some("release"),
        );
        assert_eq!(picked.expect("a beta, for want of a release").id, "b2");
    }

    #[test]
    fn an_installed_version_without_a_date_takes_anything_on_its_channel() {
        let field = field();
        let picked =
            newest_eligible(&field, "unknown", None, UpdateChannel::Release, Some("release"));
        assert_eq!(picked.expect("an update").id, "release-new");
    }

    #[test]
    fn the_policy_only_ever_rises() {
        assert_eq!(effective(UpdateChannel::Release, Some("alpha")), UpdateChannel::Alpha);
        assert_eq!(effective(UpdateChannel::Alpha, Some("release")), UpdateChannel::Alpha);
        assert_eq!(effective(UpdateChannel::Beta, None), UpdateChannel::Beta);
        assert!(allows("beta", UpdateChannel::Beta, None));
        assert!(!allows("alpha", UpdateChannel::Beta, None));
    }
}
