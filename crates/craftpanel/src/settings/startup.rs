use serde::{Deserialize, Deserializer, Serialize};

use super::runtimes::JavaRuntime;
use super::ServerRow;
use crate::auth::error::{Failure, Result};
use crate::model::{JreVendor, LoaderId};

pub const MIN_MEMORY_MIB: u32 = 512;

const PANEL_MEMORY_FLAGS: [&str; 4] = ["-Xmx", "-Xms", "-XX:MaxRAM", "-XX:MaxHeapSize"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StartupOptions {
    pub java_version: Option<u32>,
    pub jre_vendor: Option<JreVendor>,
    pub java_path: Option<String>,
    pub memory_mib: u32,
    pub memory_max_mib: u32,
    pub extra_flags: Vec<String>,
    pub startup_command: String,
    pub original_invocation: String,
    pub managed_flags: Vec<String>,
    pub stripped_flags: Vec<String>,
    pub restart_required: bool,
}

impl StartupOptions {
    pub fn dropped(mut self, flags: Vec<String>) -> Self {
        self.stripped_flags = flags;
        self
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StartupOptionsPatch {
    #[serde(deserialize_with = "present")]
    pub java_version: Option<Option<u32>>,
    #[serde(deserialize_with = "present")]
    pub jre_vendor: Option<Option<JreVendor>>,
    pub memory_mib: Option<u32>,
    #[serde(deserialize_with = "present")]
    pub startup_command: Option<Option<String>>,
}

fn present<'de, T, D>(deserializer: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Launch {
    pub jar: &'static str,
    pub trailing: &'static [&'static str],
}

const NOGUI: &[&str] = &["nogui"];
const NOTHING: &[&str] = &[];

pub fn launch_of(loader: Option<LoaderId>) -> Launch {
    match loader {
        Some(LoaderId::Velocity) => Launch { jar: "server.jar", trailing: NOTHING },
        Some(LoaderId::Quilt) => Launch { jar: "quilt-server-launch.jar", trailing: NOGUI },
        _ => Launch { jar: "server.jar", trailing: NOGUI },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub java_major: Option<u32>,
    pub jre_vendor: Option<JreVendor>,
    pub memory_mib: u32,
    pub extra_flags: Vec<String>,
    pub stripped_flags: Vec<String>,
}

pub fn view(
    server: &ServerRow,
    runtimes: &[JavaRuntime],
    memory_max_mib: u32,
    restart_required: bool,
) -> StartupOptions {
    let launch = launch_of(server.loader);
    let chosen = super::runtimes::pick(
        runtimes,
        server.java_major,
        server.jre_vendor,
        server.game_version.as_deref(),
    );
    let java = chosen.and_then(|runtime| runtime.path.clone());
    let managed = managed_flags(server.memory_mib);

    StartupOptions {
        java_version: server.java_major,
        jre_vendor: server.jre_vendor,
        java_path: java.clone(),
        memory_mib: server.memory_mib,
        memory_max_mib,
        extra_flags: server.extra_flags.clone(),
        startup_command: render(&argv(java.as_deref(), &server.extra_flags, launch)),
        original_invocation: render(&argv(java.as_deref(), &[], launch)),
        managed_flags: managed,
        stripped_flags: Vec::new(),
        restart_required,
    }
}

pub fn argv(java_path: Option<&str>, extra: &[String], launch: Launch) -> Vec<String> {
    let mut out = vec![java_path.unwrap_or("java").to_owned()];
    out.extend(extra.iter().cloned());
    out.push("-jar".to_owned());
    out.push(launch.jar.to_owned());
    out.extend(launch.trailing.iter().map(|argument| (*argument).to_owned()));
    out
}

pub fn managed_flags(memory_mib: u32) -> Vec<String> {
    vec![format!("-Xmx{memory_mib}M")]
}

pub fn plan(patch: &StartupOptionsPatch, server: &ServerRow, memory_mib: u32) -> Result<Change> {
    let mut change = Change {
        java_major: match patch.java_version {
            Some(chosen) => chosen,
            None => server.java_major,
        },
        jre_vendor: match patch.jre_vendor {
            Some(chosen) => chosen,
            None => server.jre_vendor,
        },
        memory_mib,
        extra_flags: server.extra_flags.clone(),
        stripped_flags: Vec::new(),
    };

    if let Some(command) = &patch.startup_command {
        let (flags, stripped) = read_command(command.as_deref(), launch_of(server.loader))?;
        change.extra_flags = flags;
        change.stripped_flags = stripped;
    }

    Ok(change)
}

fn read_command(command: Option<&str>, launch: Launch) -> Result<(Vec<String>, Vec<String>)> {
    let Some(command) = command else {
        return Ok((Vec::new(), Vec::new()));
    };
    if command.contains(['\n', '\r', '\0']) {
        return Err(refuse("a startup command is one line"));
    }
    let tokens = tokenise(command)?;
    if tokens.is_empty() {
        return Err(refuse("a startup command cannot be empty"));
    }

    let mut kept = Vec::new();
    let mut stripped = Vec::new();
    let mut rest = tokens.as_slice();

    if rest.first().is_some_and(|token| !token.starts_with('-')) {
        rest = &rest[1..];
    }
    if rest.len() >= launch.trailing.len() {
        let (head, tail) = rest.split_at(rest.len() - launch.trailing.len());
        if tail.iter().zip(launch.trailing).all(|(token, expected)| token == expected) {
            rest = head;
        }
    }

    let mut at = 0;
    while at < rest.len() {
        let token = &rest[at];
        if token == "-jar" {
            at += if rest.get(at + 1).is_some_and(|jar| jar == launch.jar) { 2 } else { 1 };
            continue;
        }
        if is_panel_memory_flag(token) || !token.starts_with('-') {
            stripped.push(token.clone());
        } else {
            kept.push(token.clone());
        }
        at += 1;
    }

    Ok((kept, stripped))
}

fn is_panel_memory_flag(token: &str) -> bool {
    PANEL_MEMORY_FLAGS.iter().any(|flag| token.starts_with(flag))
}

fn refuse(message: &'static str) -> Failure {
    Failure::bad_request("invalid_startup_command", message)
}

fn tokenise(command: &str) -> Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut letters = command.chars();

    while let Some(letter) = letters.next() {
        match (quote, letter) {
            (None, ' ' | '\t') => {
                if started {
                    tokens.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, '\'' | '"') => {
                quote = Some(letter);
                started = true;
            }
            (Some(open), _) if letter == open => quote = None,
            (Some('\''), _) => current.push(letter),
            (_, '\\') => {
                current.push(letters.next().unwrap_or('\\'));
                started = true;
            }
            _ => {
                current.push(letter);
                started = true;
            }
        }
    }

    if quote.is_some() {
        return Err(refuse("a quote in the startup command is never closed"));
    }
    if started {
        tokens.push(current);
    }
    Ok(tokens)
}

pub fn render(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|token| {
            if token.is_empty() || token.contains([' ', '\t', '\'', '"', '\\']) {
                format!("\"{}\"", token.replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                token.clone()
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Id, ServerStatus};

    fn a_server(loader: LoaderId, extra: &[&str]) -> ServerRow {
        ServerRow {
            id: Id::new(),
            owner_id: Id::new(),
            name: "Survival".to_owned(),
            status: ServerStatus::Available,
            loader: Some(loader),
            loader_version: Some("60".to_owned()),
            game_version: Some("1.21.8".to_owned()),
            memory_mib: 4096,
            java_major: Some(21),
            jre_vendor: None,
            extra_flags: extra.iter().map(|flag| (*flag).to_owned()).collect(),
            restart_required: false,
        }
    }

    fn patched(command: &str) -> StartupOptionsPatch {
        StartupOptionsPatch {
            startup_command: Some(Some(command.to_owned())),
            ..StartupOptionsPatch::default()
        }
    }

    #[test]
    fn xmx_is_the_panels_and_no_typed_command_can_take_it_over() {
        let server = a_server(LoaderId::Paper, &[]);
        let typed = "java -Xmx16384M -Xms16384M -XX:MaxRAMPercentage=90 -XX:MaxHeapSize=99g \
                     -XX:+UseG1GC -jar server.jar nogui";

        let change = plan(&patched(typed), &server, 4096).unwrap();

        assert_eq!(change.extra_flags, ["-XX:+UseG1GC"], "only the harmless flag survives");
        assert_eq!(
            change.stripped_flags,
            ["-Xmx16384M", "-Xms16384M", "-XX:MaxRAMPercentage=90", "-XX:MaxHeapSize=99g"],
            "and the user is told what went"
        );

        let after = ServerRow { extra_flags: change.extra_flags.clone(), ..server };
        let shown = view(&after, &[], 8192, false).dropped(change.stripped_flags.clone());

        let heap: Vec<&str> = shown
            .startup_command
            .split(' ')
            .filter(|token| is_panel_memory_flag(token))
            .collect();
        assert!(heap.is_empty(), "no heap flag in the field the page edits: {heap:?}");
        assert_eq!(shown.managed_flags, ["-Xmx4096M"], "it is named beside the field instead");
        assert_eq!(shown.stripped_flags, change.stripped_flags, "9.4 answers with what it took");
    }

    #[test]
    fn what_the_page_was_handed_comes_back_without_a_complaint() {
        let server = a_server(LoaderId::Paper, &["-XX:+UseG1GC", "-Dsome.thing=1"]);
        let shown = view(&server, &[], 16384, false);

        let change = plan(&patched(&shown.startup_command), &server, 8192).unwrap();

        assert_eq!(change.extra_flags, server.extra_flags, "his flags survive untouched");
        assert!(change.stripped_flags.is_empty(), "and nothing is reported: {change:?}");
        assert_eq!(change.memory_mib, 8192, "the slider alone decides the heap");

        let back = plan(&patched(&shown.original_invocation), &server, 8192).unwrap();
        assert!(back.extra_flags.is_empty());
        assert!(back.stripped_flags.is_empty(), "{back:?}");
    }

    #[test]
    fn the_memory_flags_are_caught_whatever_they_are_written_as() {
        for flag in [
            "-Xmx4G",
            "-Xmx4096m",
            "-Xms512M",
            "-XX:MaxRAMPercentage=75",
            "-XX:MaxRAMFraction=2",
            "-XX:MaxHeapSize=4g",
        ] {
            assert!(is_panel_memory_flag(flag), "{flag} would slip past");
        }
        for flag in ["-XX:+UseG1GC", "-Dfile.encoding=UTF-8", "-Xss1M", "-XX:MaxMetaspaceSize=1g"] {
            assert!(!is_panel_memory_flag(flag), "{flag} is not the heap");
        }
    }

    #[test]
    fn the_parts_the_panel_chooses_are_dropped_without_being_reported() {
        let server = a_server(LoaderId::Paper, &[]);
        let change = plan(&patched("java -jar server.jar nogui"), &server, 4096).unwrap();

        assert!(change.extra_flags.is_empty());
        assert!(change.stripped_flags.is_empty(), "nothing was thrown away: {change:?}");
    }

    #[test]
    fn anything_that_is_not_a_flag_falls_away_and_says_so() {
        let server = a_server(LoaderId::Paper, &[]);
        let change =
            plan(&patched("java -XX:+UseG1GC -jar mine.jar && echo hi"), &server, 4096).unwrap();

        assert_eq!(change.extra_flags, ["-XX:+UseG1GC"]);
        assert_eq!(
            change.stripped_flags,
            ["mine.jar", "&&", "echo", "hi"],
            "another jar, a shell operator and its arguments are not flags"
        );
    }

    #[test]
    fn a_command_that_cannot_be_taken_apart_is_a_four_hundred() {
        let server = a_server(LoaderId::Paper, &[]);
        for bad in ["", "   ", "java -Dname=\"unclosed", "java\n-jar server.jar"] {
            let refusal = plan(&patched(bad), &server, 4096).unwrap_err();
            assert_eq!(refusal.code(), "invalid_startup_command", "{bad:?}");
        }
    }

    #[test]
    fn null_puts_the_command_back_to_the_loaders_own() {
        let server = a_server(LoaderId::Paper, &["-XX:+UseG1GC"]);
        let patch =
            StartupOptionsPatch { startup_command: Some(None), ..StartupOptionsPatch::default() };

        let change = plan(&patch, &server, 4096).unwrap();
        assert!(change.extra_flags.is_empty());
        assert!(change.stripped_flags.is_empty());
    }

    #[test]
    fn a_field_left_out_is_not_a_field_set_to_null() {
        let server = a_server(LoaderId::Paper, &["-XX:+UseG1GC"]);

        let empty: StartupOptionsPatch = serde_json::from_str("{}").unwrap();
        let kept = plan(&empty, &server, 4096).unwrap();
        assert_eq!(kept.java_major, Some(21), "left out means unchanged");
        assert_eq!(kept.extra_flags, ["-XX:+UseG1GC"]);

        let cleared: StartupOptionsPatch =
            serde_json::from_str(r#"{"java_version": null}"#).unwrap();
        let automatic = plan(&cleared, &server, 4096).unwrap();
        assert_eq!(automatic.java_major, None, "null means choose again automatically");
        assert_eq!(automatic.extra_flags, ["-XX:+UseG1GC"], "and touches nothing else");
    }

    #[test]
    fn the_default_command_is_the_typed_one_without_the_users_flags() {
        let server = a_server(LoaderId::Paper, &["-XX:+UseG1GC", "-Dsome.thing=1"]);
        let shown = view(&server, &[], 8192, false);

        assert_eq!(shown.startup_command, "java -XX:+UseG1GC -Dsome.thing=1 -jar server.jar nogui");
        assert_eq!(shown.original_invocation, "java -jar server.jar nogui");
        assert_eq!(shown.managed_flags, ["-Xmx4096M"], "the heap is beside the field, not in it");
    }

    #[test]
    fn a_proxy_starts_without_nogui() {
        let proxy = a_server(LoaderId::Velocity, &[]);
        let shown = view(&proxy, &[], 8192, false);

        assert_eq!(shown.startup_command, "java -jar server.jar");
        assert!(!shown.startup_command.contains("nogui"), "a proxy has no console to switch off");

        let change = plan(&patched("java -jar server.jar"), &proxy, 4096).unwrap();
        assert!(change.stripped_flags.is_empty(), "{change:?}");
    }

    #[test]
    fn quoting_survives_the_trip_out_and_back() {
        assert_eq!(
            tokenise(r#"java -Dpath="/srv/my server/lib" -jar server.jar"#).unwrap(),
            ["java", "-Dpath=/srv/my server/lib", "-jar", "server.jar"]
        );
        assert_eq!(tokenise("java   -a    -b").unwrap(), ["java", "-a", "-b"]);
        assert_eq!(tokenise(r"java -Dx=a\ b").unwrap(), ["java", "-Dx=a b"]);
        assert_eq!(tokenise("java ''").unwrap(), ["java", ""]);

        let rendered = render(&["-Dpath=/srv/my server".to_owned(), "-Xmx1M".to_owned()]);
        assert_eq!(rendered, "\"-Dpath=/srv/my server\" -Xmx1M");
        assert_eq!(tokenise(&rendered).unwrap(), ["-Dpath=/srv/my server", "-Xmx1M"]);
    }

    #[test]
    fn the_runtime_the_panel_found_is_the_one_in_the_command() {
        use crate::settings::runtimes::{JavaRuntime, Source};

        let runtimes = vec![JavaRuntime {
            major: 21,
            vendor: JreVendor::Temurin,
            version: "21.0.4".to_owned(),
            path: Some("/usr/lib/jvm/temurin-21/bin/java".to_owned()),
            source: Source::System,
            installed: true,
        }];

        let shown = view(&a_server(LoaderId::Paper, &[]), &runtimes, 8192, false);
        assert_eq!(shown.java_path.as_deref(), Some("/usr/lib/jvm/temurin-21/bin/java"));
        assert!(shown.startup_command.starts_with("/usr/lib/jvm/temurin-21/bin/java -jar"));
    }
}
