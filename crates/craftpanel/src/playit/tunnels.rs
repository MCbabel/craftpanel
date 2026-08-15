use serde::{Deserialize, Serialize};

use super::http::{Http, Result};
use super::Secret;

const LOCAL_IP: &str = "127.0.0.1";
const NAME_LIMIT: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Rust,
    Java,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressKind {
    Auto,
    Ip4,
    Ip6,
    Addr4,
    Addr6,
    Domain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    pub address: String,
    pub kind: AddressKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub id: String,
    pub addresses: Vec<Address>,
    pub trouble: Vec<String>,
}

impl View {
    pub fn is_online(&self) -> bool {
        !self.addresses.is_empty() && self.trouble.is_empty()
    }

    pub fn detail(&self) -> Option<String> {
        (!self.trouble.is_empty()).then(|| self.trouble.join(" "))
    }
}

pub async fn create(
    http: &Http,
    secret: &Secret,
    form: Form,
    agent_id: &str,
    name: &str,
    local_port: u16,
) -> Result<String> {
    #[derive(Deserialize)]
    struct ObjectId {
        id: String,
    }

    let body = create_body(form, agent_id, name, local_port);
    let created: ObjectId = http.call("/v1/tunnels/create", &body, Some(secret)).await?;
    Ok(created.id)
}

pub async fn list(http: &Http, secret: &Secret) -> Result<Vec<View>> {
    let body: Wire = http.call("/v1/tunnels/list", &serde_json::json!({}), Some(secret)).await?;
    Ok(body.tunnels.into_iter().map(View::from).collect())
}

pub async fn delete(http: &Http, secret: &Secret, tunnel_id: &str) -> Result<()> {
    let body = serde_json::json!({ "tunnel_id": tunnel_id });
    match http.call::<_, serde_json::Value>("/tunnels/delete", &body, Some(secret)).await {
        Ok(_) => Ok(()),
        Err(err) if err.named("TunnelNotFound") => Ok(()),
        Err(err) => Err(err),
    }
}

pub async fn rundata(http: &Http, secret: &Secret) -> Result<RunData> {
    http.call("/v1/agents/rundata", &serde_json::json!({}), Some(secret)).await
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunData {
    pub agent_id: String,
    pub permissions: Permissions,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Permissions {
    pub is_self_managed: bool,
    pub has_premium: bool,
    pub account_status: AccountStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountStatus {
    #[serde(rename = "guest")]
    Guest,
    #[serde(rename = "email-not-verified")]
    EmailNotVerified,
    #[serde(rename = "verified")]
    Verified,
}

impl AccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Guest => "guest",
            Self::EmailNotVerified => "email_not_verified",
            Self::Verified => "verified",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "guest" => Some(Self::Guest),
            "email_not_verified" => Some(Self::EmailNotVerified),
            "verified" => Some(Self::Verified),
            _ => None,
        }
    }
}

pub fn create_body(form: Form, agent_id: &str, name: &str, local_port: u16) -> serde_json::Value {
    let protocol = serde_json::json!({ "type": "tunnel-type", "details": "minecraft-java" });
    let endpoint = serde_json::json!({
        "type": "region",
        "details": { "region": "global", "port": null },
    });
    let origin = serde_json::json!({
        "type": "agent",
        "data": {
            "agent_id": agent_id,
            "config": { "fields": [
                { "name": "local_ip", "value": LOCAL_IP },
                { "name": "local_port", "value": local_port.to_string() },
            ] },
        },
    });

    match form {
        Form::Rust => serde_json::json!({
            "ports": protocol,
            "origin": origin,
            "enabled": true,
            "alloc": endpoint,
            "name": tunnel_name(name),
            "firewall_id": null,
        }),
        Form::Java => serde_json::json!({
            "name": tunnel_name(name),
            "protocol": protocol,
            "origin": origin,
            "endpoint": endpoint,
            "enabled": true,
            "firewall_id": null,
        }),
    }
}

fn tunnel_name(name: &str) -> String {
    let kept: String = name
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(NAME_LIMIT)
        .collect();
    let kept = kept.trim().to_owned();
    if kept.is_empty() {
        "craftpanel".to_owned()
    } else {
        kept
    }
}

#[derive(Deserialize)]
struct Wire {
    tunnels: Vec<WireTunnel>,
}

#[derive(Deserialize)]
struct WireTunnel {
    id: String,
    #[serde(default)]
    user_enabled: bool,
    #[serde(default)]
    offline_reasons: Option<Vec<String>>,
    #[serde(default)]
    connect_addresses: Vec<serde_json::Value>,
    #[serde(default)]
    public_allocations: Vec<serde_json::Value>,
}

fn each<T: serde::de::DeserializeOwned>(raw: Vec<serde_json::Value>) -> Vec<T> {
    raw.into_iter().filter_map(|entry| serde_json::from_value(entry).ok()).collect()
}

#[derive(Deserialize)]
#[serde(tag = "type", content = "value")]
enum WireAddress {
    #[serde(rename = "auto")]
    Auto { address: String },
    #[serde(rename = "ip4")]
    Ip4 { address: String, default_port: u16 },
    #[serde(rename = "ip6")]
    Ip6 { address: String, default_port: u16 },
    #[serde(rename = "addr4")]
    Addr4 { address: String },
    #[serde(rename = "addr6")]
    Addr6 { address: String },
    #[serde(rename = "domain")]
    Domain { address: String },
}

#[derive(Deserialize)]
#[serde(tag = "type", content = "details")]
enum WireAllocation {
    #[serde(rename = "PortAllocation")]
    PortAllocation {
        #[serde(default)]
        expire_notice: Option<ExpireNotice>,
    },
}

#[derive(Deserialize)]
struct ExpireNotice {
    disable_at: String,
    reason: String,
}

impl From<WireTunnel> for View {
    fn from(wire: WireTunnel) -> Self {
        let addresses = each::<WireAddress>(wire.connect_addresses)
            .into_iter()
            .map(Address::of)
            .collect();

        let mut trouble: Vec<String> = wire
            .offline_reasons
            .unwrap_or_default()
            .iter()
            .map(|reason| offline(reason))
            .collect();

        if !wire.user_enabled {
            trouble.push("The tunnel is switched off on playit.gg.".to_owned());
        }

        for allocation in each::<WireAllocation>(wire.public_allocations) {
            let WireAllocation::PortAllocation { expire_notice: Some(notice) } = allocation else {
                continue;
            };
            trouble.push(expiry(&notice));
        }

        Self { id: wire.id, addresses, trouble }
    }
}

impl Address {
    fn of(wire: WireAddress) -> Self {
        let (kind, address) = match wire {
            WireAddress::Auto { address } => (AddressKind::Auto, address),
            WireAddress::Addr4 { address } => (AddressKind::Addr4, address),
            WireAddress::Addr6 { address } => (AddressKind::Addr6, address),
            WireAddress::Domain { address } => (AddressKind::Domain, address),
            WireAddress::Ip4 { address, default_port } => {
                (AddressKind::Ip4, format!("{address}:{default_port}"))
            }
            WireAddress::Ip6 { address, default_port } => {
                (AddressKind::Ip6, format!("[{address}]:{default_port}"))
            }
        };
        Self { address, kind }
    }
}

fn offline(reason: &str) -> String {
    match reason {
        "OriginNotSet" => "The tunnel has no target on playit.gg.",
        "AgentDisabled" => "The agent is switched off on playit.gg.",
        "AgentOverLimit" => "The playit account has more agents than its plan allows.",
        "TunnelDisabled" => "The tunnel is switched off on playit.gg.",
        "PublicAllocationMissing" => "playit.gg has not given this tunnel a public port.",
        "PublicAllocationPending" => "playit.gg is still handing out the public address.",
        other => return format!("playit.gg reports {other}."),
    }
    .to_owned()
}

fn expiry(notice: &ExpireNotice) -> String {
    let why = match notice.reason.as_str() {
        "over-port-limit" => "the account is over its port limit",
        "requires-premium" => "the tunnel needs playit premium",
        other => other,
    };
    format!("This tunnel is switched off on {} because {why}.", notice.disable_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playit::http::decode;

    const LIST: &[u8] = include_bytes!("testdata/tunnels_list_client_types.json");
    const RUNDATA: &[u8] = include_bytes!("testdata/agents_rundata_client_types.json");
    const CREATE: &[u8] = include_bytes!("testdata/tunnels_create_request_client_types.json");

    #[test]
    fn our_create_body_is_the_one_playits_own_client_writes() {
        let theirs: serde_json::Value = serde_json::from_slice(CREATE).unwrap();
        let ours = create_body(
            Form::Rust,
            "11112222-3333-4444-5555-666677778888",
            "survival",
            25565,
        );

        assert_eq!(ours, theirs);
    }

    #[test]
    fn the_fallback_body_moves_two_field_names_and_nothing_else() {
        let rust = create_body(Form::Rust, "a", "survival", 25565);
        let java = create_body(Form::Java, "a", "survival", 25565);

        assert_eq!(java["protocol"], rust["ports"]);
        assert_eq!(java["endpoint"], rust["alloc"]);
        assert_eq!(java["origin"], rust["origin"]);
        assert_eq!(java["name"], rust["name"]);
        assert!(java.get("ports").is_none() && java.get("alloc").is_none());
    }

    #[test]
    fn the_body_points_at_this_machine_and_at_the_port_we_were_given() {
        let body = create_body(Form::Rust, "agent", "survival", 25571);
        let fields = &body["origin"]["data"]["config"]["fields"];

        assert_eq!(fields[0], serde_json::json!({ "name": "local_ip", "value": "127.0.0.1" }));
        assert_eq!(fields[1], serde_json::json!({ "name": "local_port", "value": "25571" }));
        assert_eq!(body["alloc"]["details"]["region"], "global");
        assert_eq!(body["ports"]["details"], "minecraft-java");
    }

    #[test]
    fn a_name_playit_would_refuse_is_cut_down_before_it_is_sent() {
        assert_eq!(tunnel_name("survival"), "survival");
        assert_eq!(tunnel_name("Renée's world — south"), "Rene's world  south");
        assert_eq!(tunnel_name(&"x".repeat(80)).len(), NAME_LIMIT);
        assert_eq!(tunnel_name("🎮🎮"), "craftpanel");
        assert!(tunnel_name(&"ä".repeat(40)).is_ascii());
    }

    #[test]
    fn all_six_address_shapes_come_through_in_playits_order() {
        let list: Wire = decode(LIST).unwrap();
        let view = View::from(list.tunnels.into_iter().next().unwrap());

        assert_eq!(
            view.addresses,
            vec![
                Address {
                    address: "quiet-forest.gl.at.ply.gg".to_owned(),
                    kind: AddressKind::Auto
                },
                Address { address: "147.185.221.24:31245".to_owned(), kind: AddressKind::Addr4 },
                Address { address: "147.185.221.24:31245".to_owned(), kind: AddressKind::Ip4 },
                Address {
                    address: "[2602:fbaf:0:1::18]:31245".to_owned(),
                    kind: AddressKind::Ip6
                },
                Address {
                    address: "[2602:fbaf:0:1::18]:31245".to_owned(),
                    kind: AddressKind::Addr6
                },
                Address { address: "mc.example.org".to_owned(), kind: AddressKind::Domain },
            ]
        );
        assert!(view.is_online(), "{view:?}");
        assert_eq!(view.detail(), None);
        assert_eq!(view.id, "c0ffee11-2233-4455-6677-889900aabbcc");
    }

    #[test]
    fn a_shape_we_have_never_seen_drops_out_instead_of_taking_the_list_with_it() {
        let body = br#"{"status":"success","data":{"tunnels":[{"id":"t","user_enabled":true,
            "connect_addresses":[{"type":"quantum","value":{"address":"x"}},
                                 {"type":"auto","value":{"address":"a.ply.gg"}}]}]}}"#;

        let list: Wire = decode(body).unwrap();
        let view = View::from(list.tunnels.into_iter().next().unwrap());

        assert_eq!(view.addresses.len(), 1);
        assert_eq!(view.addresses[0].kind, AddressKind::Auto);
    }

    #[test]
    fn every_offline_reason_playit_has_becomes_a_sentence() {
        let body = br#"{"status":"success","data":{"tunnels":[{"id":"t","user_enabled":true,
            "offline_reasons":["PublicAllocationPending"],"connect_addresses":[]}]}}"#;

        let list: Wire = decode(body).unwrap();
        let view = View::from(list.tunnels.into_iter().next().unwrap());

        assert!(!view.is_online());
        assert_eq!(
            view.detail().unwrap(),
            "playit.gg is still handing out the public address."
        );

        for reason in [
            "OriginNotSet",
            "AgentDisabled",
            "AgentOverLimit",
            "TunnelDisabled",
            "PublicAllocationMissing",
            "PublicAllocationPending",
        ] {
            let sentence = offline(reason);
            assert!(!sentence.contains(reason), "{reason} was passed through raw: {sentence}");
            assert!(sentence.ends_with('.'), "{sentence}");
        }
        assert_eq!(offline("SomethingNew"), "playit.gg reports SomethingNew.");
    }

    #[test]
    fn a_tunnel_about_to_be_switched_off_says_when_and_why() {
        let body = br#"{"status":"success","data":{"tunnels":[{"id":"t","user_enabled":true,
            "connect_addresses":[{"type":"auto","value":{"address":"a.ply.gg"}}],
            "public_allocations":[{"type":"PortAllocation","details":{"expire_notice":
                {"disable_at":"2026-09-01T00:00:00Z","remove_at":"2026-09-08T00:00:00Z",
                 "reason":"over-port-limit"}}}]}]}}"#;

        let list: Wire = decode(body).unwrap();
        let view = View::from(list.tunnels.into_iter().next().unwrap());

        assert!(!view.is_online(), "a tunnel with an end date is not simply up");
        assert_eq!(
            view.detail().unwrap(),
            "This tunnel is switched off on 2026-09-01T00:00:00Z because the account is \
             over its port limit."
        );
    }

    #[test]
    fn rundata_answers_the_question_the_whole_design_rests_on() {
        let run: RunData = decode(RUNDATA).unwrap();

        assert_eq!(run.agent_id, "11112222-3333-4444-5555-666677778888");
        assert!(run.permissions.is_self_managed);
        assert!(!run.permissions.has_premium);
        assert_eq!(run.permissions.account_status, AccountStatus::Guest);
        assert_eq!(run.permissions.account_status.as_str(), "guest");
    }

    #[test]
    fn the_account_status_keeps_one_spelling_between_playit_and_the_column() {
        for (theirs, ours) in [
            ("guest", AccountStatus::Guest),
            ("email-not-verified", AccountStatus::EmailNotVerified),
            ("verified", AccountStatus::Verified),
        ] {
            let parsed: AccountStatus =
                serde_json::from_value(serde_json::json!(theirs)).unwrap();
            assert_eq!(parsed, ours);
            assert_eq!(AccountStatus::parse(ours.as_str()), Some(ours));
        }
        assert_eq!(AccountStatus::parse("banned"), None);
    }
}
