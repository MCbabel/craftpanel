# playit.gg

Status: 2026-08-13. Design for wiring [playit.gg](https://playit.gg) into the panel, so that
**whoever gets a server here gets a public address that works — without anybody touching a
router.**

Every statement in this document has evidence behind it. Evidence is either a response fetched for
this document itself (the raw responses are in `crates/craftpanel/src/playit/testdata/`), or a line
in one of playit's three open repositories:

| Short name | Repository | State of the clone |
|---|---|---|
| `agent` | `github.com/playit-cloud/playit-agent` | `9e7b9a1`, "Release 1.0.10", 2026-06-08 |
| `plugin` | `github.com/playit-cloud/playit-minecraft-plugin` | `4888f44`, 2026-02-19, release `v0.2.0` |
| `api-java` | `github.com/playit-cloud/playit-api-java` | — (for comparison only) |

The agent is under **BSD-2-Clause** (`agent:LICENSE.txt`, "Copyright 2022 Developed Methods LLC").
That is compatible with GPL-3.0; whoever ships the binary has to ship the copyright notice with it.
See section 5.

**There is no public API documentation.** The index of the support pages (`playit.gg/support/`,
49 pages, fetched) contains no page about the API, about self-managed agents or about tunnel
limits. Everything that follows comes from their own clients and from our own measurements. That is
the most important caveat of the whole document: **we are building against an undocumented
interface that may change without notice.**

---

## 1. The claim flow

### 1.1 The flow, measured

Three calls, no credentials, nothing to type out:

```
1. Generate the code    (purely local, no network)
2. POST /claim/setup    → registers the code, returns the state
3. User opens https://playit.gg/claim/<code> and confirms
4. POST /claim/exchange → returns the secret key
```

Step 2 is repeated in a loop until it reports `UserAccepted`; only then is step 4 worth doing
(`agent:packages/playit-cli/src/main.rs:326-427`,
`plugin:src/main/java/gg/playit/minecraft/PlayitKeysSetup.java:74-137`).

### 1.2 The claim code

`playit-cli claim generate` produces **five random bytes, hex encoded**: ten characters:

```rust
// agent:packages/playit-cli/src/main.rs:312-316
pub fn claim_generate() -> String {
    let mut buffer = [0u8; 5];
    rand::rng().fill(&mut buffer);
    hex::encode(&buffer)
}
```

Measured (five calls): `f2b6188254`, `34ddf358a8`, `443318f9a3`, `66d5d05699`, `82f9e2bbea`.

The official Minecraft plugin takes **eight** bytes, so sixteen characters
(`plugin:PlayitKeysSetup.java:77-79`). Both are accepted. We measured ourselves which lengths
`/claim/setup` lets through:

| Length in characters | Response |
|---|---|
| 2 | `{"status":"fail","data":"InvalidCode"}` |
| 6, 7, 8, 9, 10, 11, 12, 14, 16, 18, 20 | `{"status":"success","data":"WaitingForUserVisit"}` |
| 24, 28, 30, 31, 32, 33, 64 | `{"status":"fail","data":"InvalidCode"}` |

**The server checks only the length, not the alphabet.** `zzzzzzzzzz` (ten characters, not hex) was
accepted with `WaitingForUserVisit`. The hex check sits in the client alone
(`agent:main.rs:318-324`). We generate hex anyway: 40 bits from a cryptographically usable
generator, not from `rand::random` in passing. A guessed code would mean taking over somebody
else's claim.

### 1.3 `claim url`

There is no network call behind it. The function is a string:

```rust
// agent:packages/playit-cli/src/main.rs:318-324
pub fn claim_url(code: &str) -> Result<String, CliError> {
    if hex::decode(code).is_err() { return Err(CliError::InvalidClaimCode); }
    Ok(format!("https://playit.gg/claim/{}", code,))
}
```

Measured: `./playit-cli claim url 34ddf358a8` → `https://playit.gg/claim/34ddf358a8`.

**The options `--name` and `--type` are silently dropped in 1.0.10.** `main.rs:228` destructures
`ClaimCommands::Url { claim_code, .. }` and passes on only the code; the URL is
character-for-character the same with and without the options (checked ourselves). Older guides on
the web show a URL with `?type=…&name=…`. Copy them and you copy something that no longer exists:
**the URL is `https://playit.gg/claim/<code>`, nothing else.**

The name and the type of the agent travel in the body of `/claim/setup` instead (`agent_type`), not
in the URL.

### 1.4 `POST /claim/setup`

Body (`agent:packages/api_client/src/api.rs:1624-1636`):

```json
{ "code": "34ddf358a8", "agent_type": "self-managed", "version": "craftpanel 0.1.0" }
```

`agent_type` is `"self-managed"` or `"assignable"` (`agent:api.rs:1630-1636`). **Only
`self-managed` is usable for us**, and that is the fork the whole design hangs on:

* **`self-managed`** — the agent key may manage this agent's tunnels itself. Evidence: the auth
  errors that exist only for it (`AgentNotSelfManaged`, `SelfManagedAgentCanOnlyAffectSelf`,
  `agent:api.rs:310-311`), `AgentPermissions.is_self_managed` (`:1055-1059`),
  `"allow_self_managed": true` in `agent:agent-schema-release.json` — and the fact that playit's own
  Minecraft plugin registers exactly this type and then creates a tunnel with no human involved
  (`plugin:PlayitKeysSetup.java:85`, then `plugin:PlayitManager.java:266-298`).
* **`assignable`** — that is what the interactive path `playit-cli setup` takes
  (`agent:packages/playit-cli/src/main.rs:292`). The tunnels of such an agent are managed on
  playit.gg. That a key of this type may **not** create them over the API is inferred from the name
  `AgentNotSelfManaged` and not measured — we do not need the type anyway.

So we always send `"self-managed"` and check `permissions.is_self_managed` after claiming; if it is
`false`, the interface says so instead of keeping quiet about it.

Measured responses, all complete:

| Situation | HTTP | Body | Test file |
|---|---|---|---|
| freshly registered | 200 | `{"status":"success","data":"WaitingForUserVisit"}` | `claim_setup_waiting_for_visit.json` |
| second call with the same code | 200 | the same | — |
| code two characters long | 400 | `{"status":"fail","data":"InvalidCode"}` | `claim_setup_invalid_code.json` |
| `version` 300 characters | 400 | `{"status":"fail","data":"VersionTextTooLong"}` | `claim_setup_version_too_long.json` |
| `agent_type` unknown | 400 | `{"status":"error","data":{"type":"validation","message":"failed to parse body"}}` | — |
| empty body | 400 | the same | `error_validation.json` |

The call is **idempotent**: the same code registered twice gives `WaitingForUserVisit` twice. The
state machine has four values (`agent:api.rs:1639-1644`): `WaitingForUserVisit` → `WaitingForUser` →
`UserAccepted` | `UserRejected`. The difference between the first two is "nobody has opened the
page" versus "the page is open, the click is missing"; for us both are the same waiting, and the
interface may show both the same way.

### 1.5 `POST /claim/exchange` while nothing is confirmed

Body: `{"code":"…"}`. Measured **before** and **after** `/claim/setup`, 81 times over forty minutes,
on two independent codes, the same body every time:

```
HTTP 400   {"status":"fail","data":"CodeNotFound"}
```

That is the single most important measurement of this section, because it contradicts what the
error type suggests. `ClaimExchangeError` knows five values — `CodeNotFound`, `CodeExpired`,
`UserRejected`, `NotAccepted`, `NotSetup` (`agent:api.rs:1672-1678`). You would expect `NotSetup` or
`NotAccepted`. What comes back is `CodeNotFound`, even for a code registered one second earlier with
`WaitingForUserVisit`.

**Consequence for the build:** `CodeNotFound` from `/claim/exchange` must **not** be read as "the
code is gone". Neither official client does that either: the CLI treats every `Fail` as "keep
waiting" (`agent:main.rs:406-412`), the plugin only calls `exchange` at all after `setup` has
reported `UserAccepted` (`plugin:PlayitKeysSetup.java:89-96`). **We do it like the plugin:**
`exchange` only after `UserAccepted`. Then `CodeNotFound` is a real error there again.

Nothing was confirmed for this document; it is not an account of ours. What comes after the
confirmation therefore stands here only as source evidence: `{"status":"success","data":
{"secret_key":"<hex>"}}` (`agent:api.rs:1667-1669`, `AgentSecretKey { secret_key: String }`). The
key is hex encoded; `playitd` checks it with `hex::decode`
(`agent:packages/playitd/src/daemon.rs:1118-1124`).

### 1.6 How long a code is valid

**Measured: at least forty minutes.** Two unconfirmed codes, each polled once a minute, **81
measurements** in total over forty minutes:

| Response | Hits |
|---|---|
| `/claim/setup` → `{"status":"success","data":"WaitingForUserVisit"}` | 81 |
| `/claim/exchange` → `{"status":"fail","data":"CodeNotFound"}` | 81 |
| `CodeExpired` or `InvalidCode` | **0** |

The error value exists (`ClaimSetupError::CodeExpired`, `agent:api.rs:1647-1651`) and both clients
handle it: the plugin throws the code away and generates a new one
(`plugin:PlayitKeysSetup.java:118-121`). It was simply never triggered; the upper bound is still
open.

What follows from that: **the interface must not rely on any deadline, neither on a short one nor
on this one.** It shows the URL, it polls, and when `CodeExpired` arrives it generates a new code
and a new URL without a word. Our own deadline is **15 minutes**, after which the panel-side claim
aborts, not because playit would abort then (demonstrably not), but so that no background loop runs
against somebody else's house every two seconds for an indefinite time. Anybody who needs longer
presses the button again.

### 1.7 The error format of api.playit.gg

Two levels, and they are easily confused (`agent:api.rs:331-341`):

```json
{"status":"success","data": <payload>}
{"status":"fail",   "data": "<domain error as a string>"}
{"status":"error",  "data": {"type":"…","message":…}}
```

`fail` is the expected domain outcome of a call (`InvalidCode`, `CodeNotFound`). `error` is
everything else. Measured:

| Case | HTTP | Body |
|---|---|---|
| without `Authorization` | 401 | `{"status":"error","data":{"type":"auth","message":"AuthRequired"}}` |
| `Authorization: Agent-Key deadbeef` | 401 | `{"status":"error","data":{"type":"auth","message":"InvalidAgentKey"}}` |
| unknown path | 404 | `{"status":"error","data":{"type":"path-not-found","message":{"path":"/nope/nothing"}}}` |
| GET instead of POST | 400 | `{"status":"error","data":{"type":"validation","message":"POST method required"}}` |
| unreadable body | 400 | `{"status":"error","data":{"type":"validation","message":"failed to parse body"}}` |

Note: **with `path-not-found`, `message` is an object; everywhere else a string.** Declare
`message: String` and you fail on the 404. That is why our parser takes the `error` branch as a
`serde_json::Value` and turns it into a sentence instead of typing it.

The header is **`Authorization: Agent-Key <hex>`** (`agent:packages/api_client/src/lib.rs:15`).
Next to it there is `Api-Key` for their own release pipeline
(`agent:build-scripts/submit-release.sh:8`). We use `Agent-Key` only.

**All calls are POST**, the reading ones too, even the ones with an empty body `{}`
(`agent:packages/api_client/src/http_client.rs:57`).

### 1.8 Rate limits

Twenty calls to `/info/pops` in a row, without a pause: twenty times 200. No `X-RateLimit-*`
headers, no `Retry-After`; the responses come through Cloudflare (`server: cloudflare`,
`cf-cache-status: DYNAMIC`). The client still knows `HttpClientError::TooManyRequests` for HTTP 429
(`agent:http_client.rs:74-76`), so there is a limit; it is just not documented and was not reached
at this rate.

Our intervals are chosen conservatively to match: claim 2 s (like both official clients), tunnel
reconcile 30 s. On 429 we double up to 5 minutes.

---

## 2. Tunnels — the core question

> **Can the panel create a tunnel per Minecraft server and learn its public address without a human
> clicking anything on playit.gg?**
>
> **Yes** — a human has to claim the agent once (section 1), never again after that.

### 2.1 The proof

It is not the agent that creates tunnels. In its entire source `playitd` calls exactly **one**
tunnel-related route, and that one reads:

```
$ grep -rn "tunnels_create\|tunnels_list\|agents_rundata" --include=*.rs packages/ \
      | grep -v api_client/src/api.rs
packages/playitd/src/daemon.rs:540:  if let Ok(data) = api.v1_agents_rundata().await {
packages/playitd/src/daemon.rs:803:  match api.v1_agents_rundata().await {
```

The agent **reads** its tunnels and forwards accordingly. They are created from outside. And that is
what playit's **own Minecraft plugin** does, with nothing but the agent key:

```java
// plugin:src/main/java/gg/playit/minecraft/PlayitManager.java:266-298
private String ensureTunnelExists(PlayitKeysSetup.PlayitKeys keys) throws IOException {
    var api = new ApiClient(keys.secretKey);              // Agent-Key, nothing else
    var tunnelsResult = api.v1TunnelsList();
    …
    for (AccountTunnelV1 tunnel : tunnels.tunnels())
        if (tunnel.tunnel_type() == TunnelType.MinecraftJava) {
            var addr = extractDisplayAddress(tunnel);
            if (addr != null) return addr;               // reuse an existing one
        }

    log.info("create new minecraft java tunnel");
    var create = new ReqTunnelsCreateV1(
            "Minecraft",
            new TunnelProtocol.TunnelTypeDetail(TunnelType.MinecraftJava),
            new AccountTunnelOriginCreate.Agent(new AgentOrigin(keys.agentId, new AgentTunnelConfig())),
            new CreateTunnelEndpoint.Region(new UseAllocRegion(PlayitNetwork.Global, null)),
            true, null);
    api.v1TunnelsCreate(create);

    for (int i = 0; i < 10; i++) {                        // wait up to 10 s for the address
        Thread.sleep(1000);
        …
    }
}
```

The plugin gets itself a key beforehand exactly the way we would: `claim generate`, `claim/setup`,
`claim/exchange`, agent type **`self-managed`** (`plugin:PlayitKeysSetup.java:85`). After that it
creates a tunnel with no human at all and reads out the address. **That is precisely our use case,
built by playit themselves.**

Three more pieces of evidence that the agent key of a self-managed agent is meant for this:

* `agent-schema-release.json` in the root of the agent repository, the file playit sends to
  `POST /release/agent_version` on every release (`agent:build-scripts/submit-release.sh`), contains
  `"allow_self_managed": true`.
* The auth errors `AgentNotSelfManaged` and `SelfManagedAgentCanOnlyAffectSelf`
  (`agent:api.rs:310-311`). They exist only because an agent key may call account-scoped routes,
  limited to itself.
* `TunnelConfigError::SelfManagedAgentCannotReassignTunnel` (`agent:api.rs:929-930`). So a
  self-managed agent may call `/v1/tunnels/config`; the one thing forbidden to it is attaching the
  tunnel to a *different* agent.

`AgentRunDataV1.permissions.is_self_managed` (`agent:api.rs:1055-1059`) tells us after claiming
whether we really have a self-managed agent. If not, the design is void, and the interface has to
say so instead of keeping quiet about it.

### 2.2 What is not proven yet — and how the first builder proves it

Two things cannot be measured without a real key, because authentication takes effect **before** the
body is validated (measured: `/tunnels/create` with `{}` → 401, `/claim/setup` with `{}` → 400
`validation`).

**(a) The exact body shape of `/v1/tunnels/create`. The two official clients contradict each
other.**

| | Rust agent 1.0.10 (2026-06) | Java plugin 0.2.0 (2026-02) |
|---|---|---|
| Source | `agent:api.rs:752-759` | `plugin:…/request/ReqTunnelsCreateV1.java` |
| Fields | `ports`, `origin`, `enabled`, `alloc`, `name`, `firewall_id` | `name`, `protocol`, `origin`, `endpoint`, `enabled`, `firewall_id` |
| Protocol field | `ports: {"type":"tunnel-type","details":"minecraft-java"}` | `protocol: {"type":"tunnel-type","details":"minecraft-java"}` |
| Target field | `alloc: {"type":"region","details":{"region":"global","port":null}}` | `endpoint: {"type":"region","details":{"region":"global","port":null}}` |
| Variants of the target field | `hostname`, `dedicated-ip`, `shared-ip`, `region`, `port-allocation` | `gateway`, `dedicated-ip`, `shared-ip`, `region`, `port-allocation` |

Both files are generated ("mod api is auto generated", `agent:packages/api_client/src/lib.rs:4`), so
both were correct at their time. The Rust one is four months newer. The two agree on `origin`,
`enabled`, `name`, `firewall_id` and on every value — only two field names differ.

**Task for the first builder:** with a real key, send both shapes once and store the response as a
test file. Until then the Rust shape is the default (newer source) and the Java shape the fallback
on the first `validation` error. Two attempts, no more; after that the endpoint answers
`502 upstream_unavailable` with playit's own text.

**(b) Whether `Agent-Key` is allowed on `/v1/tunnels/create`.** All the evidence says yes (2.1). A
`401` with `AgentNotSelfManaged` would be the counter-proof; then the whole design falls back to
"the admin creates the tunnels by hand on playit.gg and enters them here". That is the one
assumption in this document that can overturn the design, which is why it stands in the report and
not in a footnote.

### 2.3 How a tunnel knows where it points

Not through a local configuration file. **The target address lives on playit's side**, in the
tunnel's `agent_config`, and the agent reads it fresh on every `rundata` round:

```rust
// agent:packages/agent_core/src/network/origin_lookup.rs:203-215
let local_port = tunn.agent_config.fields.iter()
    .find(|f| f.name.eq("local_port"))
    .and_then(|v| u16::from_str(&v.value).ok())
    .or_else(|| tunn.display_address.rsplit(':').next().and_then(|p| u16::from_str(p).ok()))?;
```

The permitted field names are in the schema file that playit gets from the agent
(`agent:agent-schema-release.json`, stored here as `testdata/agent_schema_release_v1_0_10.json`):

| Field | Type | Default | applies to |
|---|---|---|---|
| `local_ip` | string (IP or hostname) | `127.0.0.1` | all tunnels |
| `local_port` | port, may be absent | — | all except `https` |
| `proxy_protocol` | `proxy-protocol-v1` \| `proxy-protocol-v2` | — | all |
| `http_port` / `https_port` | port | `80` / `443` | `https` tunnels only |

If `local_port` is missing, the agent takes the public port from `display_address`; for us that
would be wrong, because our servers listen on 25565 and up from the pool, not on the port playit
hands out. **We always set `local_ip` and `local_port` ourselves.**

> **This is the security-critical spot of the whole design.** `local_port` is a hole from the
> internet onto a port of this machine. If the value ever came out of a request, a user could
> publish the panel itself (8099), the helper socket or any other local service. **`local_port`
> comes exclusively from the server's primary port allocation (`allocations.is_primary = 1`),
> `local_ip` is the constant `127.0.0.1`.** No endpoint in this design takes a port. Introduce a
> field for it while building and you undo the isolation from the section "Permissions and
> isolation" in `docs/PLAN.md`.

### 2.4 How the panel learns the address

Two ways, both with the agent key:

* `POST /v1/tunnels/list` → `AccountTunnelsV1 { tunnels: [AccountTunnelV1] }` with
  `connect_addresses: Vec<ConnectAddress>` per tunnel (`agent:api.rs:395-409`).
* `POST /v1/agents/rundata` → `AgentRunDataV1` with `tunnels[].display_address: String`
  (`agent:api.rs:1006-1026`).

`ConnectAddress` is a sum of six cases (`agent:api.rs:673-687`): `addr4`, `addr6`, `ip4`, `ip6`,
`auto` (the name playit assigns), `domain` (your own domain). The plugin simply takes the **first
entry** and forms a display string from it (`plugin:PlayitManager.java:329-341`).

**We do not assemble the address ourselves** — neither `<name>.gl.at.ply.gg` nor `ip:port`. We take
what `connect_addresses` delivers, in the order delivered, and show all of it. The reason: the
`auto` case gives a name without a port (Minecraft Java can resolve over SRV,
`HostnameRoutingType::MinecraftJava`, `agent:api.rs:664-668`), the `ip4` case gives the address and
`default_port` separately. Guess a string out of that and you guess wrong in half the cases. The
only assembly we do is `ip:port` from the two fields playit itself kept apart
(`playit/tunnels.rs:305-311`).

**And both lists are taken in untyped and turned over entry by entry**
(`playit/tunnels.rs:219-222`). They are tagged sums that can get a seventh case any day, and serde
reads an unknown tag as a **whole broken document**. A single new address form from playit would
make the address of *every* tunnel of *every* user go empty, all at once, and without anything
here having changed. So an unknown entry drops out on its own, and the rest stay.

The shape of the name is documented anyway: `at.ply.gg` and `gl.at.ply.gg` resolve to `69.9.186.255`
(`dig`, checked ourselves), name server `ns1.playit-dns.com`; made-up subnames do **not** resolve:
the names are real records per tunnel, not a placeholder zone. `gl` stands for the global anycast of
the free tier.

After creation the address is **not there immediately**. The plugin waits up to ten seconds in
one-second steps (`plugin:PlayitManager.java:300-326`); the state in between is called
`PortAllocationStatus::Pending` or `AccountTunnelOfflineReason::PublicAllocationPending`
(`agent:api.rs:557-565`, `:416-423`). Our design turns that into a visible intermediate state
instead of a long HTTP response; see 8.8.

### 2.5 No second path

Considered and rejected: speaking the data path ourselves, the way the plugin does
(`plugin:PlayitControlChannel`, UDP 5525, their own frame formats in `agent_proto`). That would be
several thousand lines of somebody else's protocol inside our process, for zero gain: the finished
binary does it better and playit maintains it. We take the agent for the data and the API for the
management.

---

## 3. Where the agent runs

### 3.1 One process per connected user, not per server

The key belongs to the account, not to the server, and one agent serves any number of tunnels
(`AgentRunDataV1.tunnels` is a list). One agent per server would mean: one claim per server, several
agents on one free account, and playit limits exactly the number of agents ("more firewalls, ports,
and agents", `playit.gg/support/playit-premium/`).

Since "one account per panel user" there are several accounts, though, and one account is one key:

**So: exactly one `playitd` per connected user — and only as long as that user has at least one
tunnel.** An agent without a tunnel forwards nothing; it costs 4.7 MB and a process for doing
nothing. Exactly one function decides that (`Connection::tune_agent`, behind a lock), and it reads
two things: does the user have a tunnel, and is the switch from 12.10 on.

The new normal state `configured && ports.used == 0 && agent == absent` means "no server of yours
has a public address, so the tunnel service is not running" — not an error, and the interface says
exactly that sentence.

**`TOKIO_WORKER_THREADS=2` is set.** Measured against v1.0.10, started without a key (no call to
playit, no account touched), threads from `/proc/<pid>/status`:

| `TOKIO_WORKER_THREADS` | Threads | VmSize | RssAnon |
|---|---|---|---|
| not set (16 CPUs) | **18** | 40,104 kB | 580 kB |
| 4 | 6 | 15,160 kB | — |
| **2** | **4** | 11,012 kB | 456 kB |
| 1 | 3 | 8,924 kB | — |

So `playitd` does not set `worker_threads` itself, and the variable takes effect. To be honest about
it: the *resident* saving is small (~124 kB per agent, thread stacks are only reserved, never
touched); what is gained is 14 threads and 29 MB of address space per agent. **Not 1** — one
blocking operation would then stall the forwarder itself.

The running agent with a real tunnel, for comparison: 17 threads, VmRSS 4,656 kB, PSS 4,652 kB,
VmSize 74,004 kB, 6.6 s of CPU in an hour (≈0.04 %). So **~4.7 MB per connected user with a
tunnel**: ten ≈ 47 MB and 40 threads (with the variable, instead of 170), a hundred ≈ 470 MB and 400
threads. There are no limits on this machine: `pids.max = 70773` at `pids.current = 39`,
`memory.max = max`, 24 GB. The binary (5,894,088 B) lives once in `<data_dir>/cache/playit` and is
shared: the lock around `Binary::ensure` is the reason two users with their first tunnel do not
load into the same half-written file.

### 3.2 Which binary

Not `playit-cli`. Since 1.0 the CLI is only a front end that talks to the daemon over an IPC socket
(`agent:packages/playit-cli/src/client.rs`, socket `/run/playit/playitd.sock`); it does not know
`run`. What tunnels is the daemon.

Measured: the release artifact **`playit-linux-<arch>` is `playitd`**:

```
$ ./playit-daemon --help
Usage: playit-daemon [OPTIONS]
Options:
      --secret <SECRET>            Inline secret key for the daemon
      --secret-path <SECRET_PATH>  Path to the daemon secret file
      --socket-path <SOCKET_PATH>  Override the IPC socket or named pipe path
  -l, --log-path <LOG_PATH>        Path to write daemon logs to
      --platform-docker            Overrides platform registration to be docker
```

From `playit-cli` we need **nothing**. `claim generate` is three lines (1.2), `claim url` is one
(1.3), `claim/setup` and `claim/exchange` are two HTTP calls. That saves the second 5 MB file.

### 3.3 Under which account

**As `craftpanel`, the unprivileged panel account. Never as root.**

Measured that this works — the same call as `nobody`, without any permission at all:

```
$ su nobody -s /bin/bash -c '/tmp/playit-daemon --secret deadbeefdeadbeef \
      --socket-path /tmp/playittest/p.sock -l /tmp/playittest/d.log'
playitd error: Setup error: The configured playit secret is no longer valid.
exit=1
```

The daemon started, registered with the API, got `InvalidAgentKey`, wrote its log file and exited
with 1. Neither the socket path nor the log nor network access needed permissions. What it does
need: outbound **UDP 5525** to playit's control nodes
(`agent:packages/agent_core/src/agent_control/mod.rs:259-271`), HTTPS to `api.playit.gg`, and
locally `connect()` on `127.0.0.1:<serverport>`.

Why `craftpanel` and not an account of its own, `craft-playit`:

* The agent needs **no** file access to server directories. It connects to a local port, nothing
  more. A separate account would gain nothing there.
* It needs the key, and the key belongs to the panel. A separate account would mean making a file
  shareable between two accounts, one more gap, no gain.
* A separate account would require a new helper command or the misuse of `create-user`. The helper
  has a **fixed, short command vocabulary** (`docs/PLAN.md:181-191`); extending it for a tunnel
  agent would be the wrong direction. `SpawnRequest` (`crates/craftpanel-proto/src/lib.rs`) demands
  `server_id`, `supervisor_socket` and `token` and does not fit a tunnel agent.

What `craftpanel` does **not** gain by this: nothing. As `craftpanel` the agent can do exactly what
the panel can do anyway. The one new way in is the one from outside to inside, and 2.3 limits that.

**Even with one account per user, still no separate system account per agent, and what that leaves
open.** There are now N agents running under the same `craftpanel`, and all N keys are readable for
that process. A compromised panel therefore hands over N foreign playit accounts instead of one.
That is a **regression**, it follows from the requirement, and it cannot be fixed with today's
helper. It belongs said out loud, not left out.

What still holds the separation: the games run as `craft-<id>` and cannot get through a 0700
directory owned by `craftpanel`; that is already the case today
(`drwx------ craftpanel /var/lib/craftpanel/playit`), and it applies to every subdirectory as well.

### 3.4 No supervisor

A Minecraft server survives a panel update because a supervisor stands in between
(`docs/PLAN.md:216-241`). The tunnel agent does not need that:

* It holds no state. Everything it knows lives on playit's side and is fetched again at startup
  (2.3).
* It has no console anybody could lose.
* It is back in milliseconds.

**So: an ordinary child of the panel process**, `tokio::process::Command`, watched by a task with
backoff 1 s → 2 s → 4 s … up to 60 s. If the panel dies, the agent dies with it; when the panel
comes back, it starts again.

**Without `--log-path`.** The daemon writes its lines either into that file **or** to standard
error, never into both (`agent:playitd/daemon.rs:1262-1300`); measured ourselves: with the flag,
both of the child's pipes stay empty. Here the file is the worse half: under Linux nothing rotates
it (`rolling::never`, `agent:daemon.rs:1337-1347`), and it would sit in a directory only the panel
can open. So we take the lines ourselves: the last of them is the sentence section 6 promises the
admin when the daemon is not running. It arrives wrapped in color codes (`use_ansi` is hard-wired on
under Linux, `agent:daemon.rs:370`) and gets unwrapped before it lands in a response.

**The price, and it belongs said out loud:** a panel update disconnects every player connected
through playit for a few seconds. The Minecraft servers keep running, the players reconnect. That is
the difference between "the server is gone" and "the line twitched", and it is acceptable. A
supervisor for the agent would be the second-best solution to a problem that does not hurt.

### 3.5 How the key reaches the agent

**Not through `--secret`.** Arguments stand in `/proc/<pid>/cmdline` and are readable for **every**
user of this machine, including `craft-<id>`, so for every plugin on every server. Whoever has the
key manages every tunnel of the account.

**Through `--secret-path`.** What gets read is either a bare hex line or TOML with
`secret_key = "…"` (`agent:packages/playitd/src/daemon.rs:1108-1116`). We write the bare form to
**`<data_dir>/playit/<user_id>/secret`**, mode `0600`, directory `0700`, owner `craftpanel`. That is
the same protection `panel.db` has (`docs/PLAN.md:152`), and playit itself writes its file with
`0600` too (`agent:daemon.rs:1049-1052`).

Exactly two processes may read any of these keys, and both are `craftpanel`: the panel process and
the `playitd` it starts as a child.

**The socket moves along**, and that is mandatory:
`<data_dir>/playit/<user_id>/playitd.sock`. Two agents on one socket path would overwrite each
other. Measured during the trial run: `playitd` wants to give the socket to the group `playit`, does
not find it and leaves the default mode in place (`srw-rw---- craftpanel craftpanel`). Inside the
0700 directory that is out of reach for `craft-*`; without the directory it would be a gap.

**The key is not in the database.** A copy of `panel.db` — for debugging, for a backup — then
carries no way in to somebody else's service. The table remembers everything about the account
except the key itself.

**And `--secret-path` behaves differently from `--secret` with an invalid key**: that is not a side
issue but the second reason for the choice. The daemon distinguishes `SecretSource::Inline` from
`SecretSource::File`, and only the file allows the key to be supplied later
(`allows_ipc_provisioning`, `agent:packages/playitd/src/daemon.rs:239-241`):

* `--secret` with an invalid key → the daemon **exits** with code 1. That is exactly what was
  measured above.
* `--secret-path` with an invalid key → the daemon goes into `WaitingForSecret` and **stays alive**
  (`agent:packages/playitd/src/daemon.rs:561-592`). Measured with the real 1.0.10 binary: after 45 s
  and after 180 s it is still running, no exit code.

**What it waits for, though, is not the file.** The name of the function says it —
`allows_ipc_provisioning` — and the log line says it word for word: `Waiting for frontend secret
provisioning over IPC`. Measured ourselves: key file replaced with a different one while the daemon
was running (atomically, the way the panel does it), waited 180 seconds: **no new line in the log,
no second claim attempt**. A running `playitd` does not read its file a second time.

For us that means: after a new claim it is **not** enough to rewrite the file; the agent has to be
restarted, otherwise it keeps the key it came up with. That is why `Connection::adopt` stops that
user's daemon before `tune_agent` starts it again. And it means: an invalid key is **not**
recognizable to us from the process exiting; it becomes visible on our own `rundata` call, which
returns `401 InvalidAgentKey` (1.7).

---

## 4. The binary

### 4.1 Decision: fetch on demand, with a checksum built in

Do not ship it. Three reasons, in this order:

1. **Size.** The four Linux files weigh 5.3 to 6.2 MB, 22.7 MB together, in a binary whose bundle
   is already on the to-do list as too large. The promise is a `curl` command, not a small
   download, but it is also not *this* much extra for an optional feature that most installations
   never switch on.
2. **Age.** 1.0.10 is from 2026-06-08. The agent speaks a protocol with a version number
   (`proto_version: 2`) and registers with its version (`ReqProtoRegister`, `agent:api.rs:1938`);
   the API knows `AgentVersionTooOld` (`TunnelCreateErrorV1`, `agent:api.rs:848`). A shipped file
   freezes exactly the thing that is allowed to age.
3. **Licence.** BSD-2-Clause requires the copyright notice to be included in the documentation when
   shipping in binary form. Doable, but one more obligation in `COPYING.md` that goes away if we do
   not ship the file.

Only **the architecture we run on** is fetched, and only if the admin actually switches playit on.

### 4.2 How the file is verified

**There is no signature and no `.sha256` file next to the release.** Checked: the 32 assets of
`v1.0.10` contain no checksum file. For Debian and Alpine there are signed package repositories
(`playit-cloud.github.io/ppa`, GPG key retrievable, 1680 bytes), but they only go up to `0.15.x`:
no `1.0.10` in the `Packages` index (checked ourselves, zero hits). So the `apt` route is out, and
with it the signature.

What does exist: **GitHub delivers a `digest` field per asset** in the release API. Checked
ourselves: the value matches the downloaded file byte for byte:

```
$ sha256sum playit-daemon
2df7d9f10227ab312b1ad341853db4e8a8243df5cfcdbae58713a4271711c339  playit-daemon
$ …/releases/latest → assets[playit-linux-amd64].digest
"sha256:2df7d9f10227ab312b1ad341853db4e8a8243df5cfcdbae58713a4271711c339"
```

**The rule:** the checksum lives **in our source code**, not in a response from the network.

```rust
const RELEASE: &str = "v1.0.10";
const BINARIES: &[(&str, &str, &str)] = &[
    ("x86_64",  "playit-linux-amd64",   "2df7d9f10227ab312b1ad341853db4e8a8243df5cfcdbae58713a4271711c339"),
    ("aarch64", "playit-linux-aarch64", "4c0db3e7b3a8158e249441c2f0b73f54e83429395890c7b1ca45fd7a6303d763"),
    ("armv7",   "playit-linux-armv7",   "92ec60988b1246e07ac090c663128bd04bdc0d7ff388db520e1ff7bb4e5003e0"),
    ("i686",    "playit-linux-i686",    "d7215f3995e486bc231b3b542aa5f1ac6b0d604f8dae97bb14a9a64b49b3ed50"),
];
```

All four values above come from the release API and are stored as
`testdata/github_release_v1_0_10.json`; the `amd64` value is additionally checked against the file
actually downloaded.

Why not just take the `digest` field at runtime: then GitHub would be checking GitHub. A built-in
sum binds us to **the version we built and tested against**, exactly what
`crates/craftpanel/src/loaders/checksum.rs` already does for the server jars. When raising the
version you fetch the new sum from the same API and enter it; the response for that is already there
as a test file.

If the sum does not match, the file is **deleted** and the error stands. No "maybe it works anyway".

### 4.3 Where to, and how often

`<data_dir>/cache/playit/playit-<version>-<arch>`, mode `0700`, owner `craftpanel`. `cache_dir()`
already exists (`crates/craftpanel/src/config.rs`). On a version change the new file sits next to
the old one, and the old one is removed after the first successful start of the new one.

Timeouts as everywhere else: connect 5 s, total download time 300 s. A hanging download must not
block an endpoint: it runs in the background, and the status endpoint shows `binary.state`.

---

## 5. Limits of the free tier

### 5.1 Four ports

Word for word from `playit.gg/support/playit-premium/`, fetched 2026-08-13:

> "With playit premium the number of ports you can allocate jumps from **4 to 16**. You also get
> more firewall rules and agents."

A Minecraft Java tunnel occupies one port — that is inferred, not measured: Minecraft Java is one
TCP port, and `AccountTunnelV1.port_count` for a tunnel of this type is 1 accordingly
(`agent:api.rs:395-409`). **So four servers with a public address at the same time, per user
account, not per panel.** Since "one account per panel user" everybody has their own four (sixteen
with premium), read from **their** row; `store::used` and `claim_slot` count only their tunnels, in
the same transaction as before.

That decides the interface: "one tunnel per server" works, but not for any number of servers of one
user. The status endpoint delivers `used`, `limit` and `for_others`, and `POST …/playit` answers
`409 playit_port_limit` when nothing is free any more, instead of letting playit say it, with the
sentence "your playit.gg account has no free port left (4 of 4 in use)". Before, the sentence was
impersonal, because a stranger could have used up the port; now it names whose budget is full.

For completeness, because it carries the contract's permission table: the rule that an editor may
not create an address now rests **solely** on "it puts the server on the open internet". The second
half of the old reasoning — "and it is one of four panel-wide ports, so somebody else's port too" —
no longer applies. The rule stays, its reasoning is weaker and still has to carry.

The API knows the limit and reports it: `PortAllocationStatus::AccountPortLimitReached`
(`agent:api.rs:557-565`), `TunnelCreateErrorV1::RequiresPlayitPremium` (`:849`) and
`DisabledReason::OverPortLimit` (`:648-653`).

### 5.2 Global anycast, no region

> "Free tunnels on playit.gg are 'Global Anycast'. While they work quite well, routing is not
> always optimal. […] Regional tunnels fix this […] You can select your region when creating a
> new tunnel."

So on creation we set the region **`global`**, like the plugin (`plugin:PlayitManager.java:293`,
`PlayitNetwork.Global`). A region picker in our interface would be a surface that produces nothing
but error messages on a free account (`TunnelCreateErrorV1::RegionRequiresPlayitPremium`,
`agent:api.rs:845`).

The 22 locations are publicly retrievable and available without signing in: `POST /info/pops`, the
response is stored as `testdata/info_pops.json`. It is there as evidence, not because we need it.

### 5.3 Address form

The free global tunnel gets a name under `gl.at.ply.gg`; the zone exists and resolves (2.4). That a
bare IPv4 with a default port sits next to it follows from the variants of `ConnectAddress` (`ip4`,
`addr4`) and is not measured. Hence the rule from 2.4: **whatever is in `connect_addresses` gets
shown — all of it, in playit's order, and none of it assembled.**

### 5.4 Rate limits

See 1.8: none measured, but 429 handling is provided for in the official client.

### 5.5 Account states that affect us

`AgentAccountStatus` (`agent:api.rs:1793-1811`) and `AccountStatus` (`:1061-1069`): `guest`,
`email-not-verified`, `verified`, `ready`, plus `banned`, `agent-disabled`, `agent-over-limit`,
`has-message`, `account-delete-scheduled`. An account created through `claim` **without** an e-mail
sign-up is a **guest account**; the plugin then warns and offers a sign-up link
(`plugin:PlayitManager.java:160-190`). We do the same: `account_status` is in the status endpoint,
and with `guest` a sentence next to it says that a guest account can be lost.

What we do **not** do: generate the sign-up link ourselves through `POST /login/guest`. That is an
account endpoint, not a tunnel endpoint, and the admin can sign in at playit like everybody else.

---

## 6. What happens on failure

The principle, the same as everywhere in the panel: **nothing dies, and nobody guesses.** A broken
tunnel must not touch a Minecraft server. The local port stays reachable, whatever playit does.

| Case | How the panel notices | What the panel does | What the user sees |
|---|---|---|---|
| **api.playit.gg unreachable** | timeout or connection error in our HTTP layer | delay the reconcile, backoff 30 s → 5 min; **the last known address stays** | the address with the note "last confirmed … ago"; no wall of errors |
| **playit answers 429** | HTTP 429 | double the interval up to 5 min | as above |
| **key invalid** (account deleted, agent removed, `reset`) | our own `rundata` call: `{"status":"error","data":{"type":"auth","message":"InvalidAgentKey"}}`, HTTP 401 — measured. **Not** from the process exiting: with `--secret-path` `playitd` stays alive and waits (3.5) | state `failed`, leave the agent running, leave the key file in place, throttle the reconcile to 5 min | Their account page: "Your playit.gg claim is no longer valid. Connect again." with a button. The button has to **disconnect** first (8.5, `?tunnels=keep`) and then claim again: as long as a key is present, 8.2 answers `409 playit_already_claimed`, and a running agent does not accept a new key file (3.5). Server page: address grayed out, reason in plain words |
| **tunnel deleted on playit.gg** | tunnel ID missing from `/v1/tunnels/list` | row to `missing`, do **not** silently recreate | "The tunnel was removed on playit.gg." plus a "Create again" button |
| **tunnel offline** | `AccountTunnelOfflineReason` (`agent:api.rs:416-423`): `OriginNotSet`, `AgentDisabled`, `AgentOverLimit`, `TunnelDisabled`, `PublicAllocationMissing`, `PublicAllocationPending` | translate the reason and show it; with `PublicAllocationPending` keep waiting | one plain sentence per reason, not the identifier |
| **port over the account limit** | `ExpireNotice { disable_at, remove_at, reason: OverPortLimit }` (`agent:api.rs:641-653`) | show the date, delete nothing | "This tunnel will be switched off on …, because the account is over its port limit." |
| **agent dead** | child process exited | restart with backoff 1 s → 60 s; after five failed attempts in a row state `failed` and quiet | "The tunnel service is not running." with the last line of its log |
| **binary missing or checksum wrong** | our own comparison | delete the file, `binary.state = failed`, **no** second attempt on its own | "The tunnel service could not be loaded: the checksum does not match." |
| **panel restarted** | — | restart the agent, reconcile the tunnels | a few seconds of interruption for connected players (3.4) |
| **server deleted** | `ON DELETE CASCADE` | delete the tunnel at playit so the port is freed | — |
| **player gets "Unknown host"** | not detectable — that happens at the player's end | Nothing. But the interface shows **both** addresses (name and IP:port), so the way out sits right next to it | playit has its own help page for this (`/support/minecraft-java-unknown-host/`): some ISP resolvers return nothing for `*.ply.gg` |

One case is explicitly **not** a failure: **playit is not set up at all.** Then there is no warning,
no red box and no note on a server page, only an unobtrusive section saying that this exists. A
panel that nags about an optional feature is a panel that annoys.

---

## 7. The design: the database

Two migrations. `0006_playit.sql` created three tables for **one** panel-wide account;
`0008_playit_per_user.sql` turns that into one account per user and adopts the old one. First what
0006 left behind:

```sql
-- The account. One row, like panel_settings. The secret key deliberately does
-- not live here but in <data_dir>/playit/secret with 0600: a copy of the
-- database must not carry access to somebody else's service with it.
CREATE TABLE playit_account (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    -- playit's own UUID for the agent, not a ULID. It comes from
    -- /v1/agents/rundata and is sent along when a tunnel is created.
    agent_id         TEXT,
    account_status   TEXT    CHECK (account_status IN ('guest', 'email_not_verified', 'verified')),
    is_self_managed  INTEGER NOT NULL DEFAULT 0 CHECK (is_self_managed IN (0, 1)),
    has_premium      INTEGER NOT NULL DEFAULT 0 CHECK (has_premium IN (0, 1)),
    -- The claim in progress. All three set together or all three NULL.
    claim_code       TEXT,
    claim_state      TEXT    CHECK (claim_state IN ('waiting_for_visit', 'waiting_for_user',
                                                    'accepted', 'rejected')),
    claim_started_at TEXT,
    -- Last successful reconcile and last error in plain words.
    checked_at       TEXT,
    last_error       TEXT,
    updated_at       TEXT    NOT NULL,
    CHECK ((claim_code IS NULL) = (claim_state IS NULL)),
    CHECK ((claim_code IS NULL) = (claim_started_at IS NULL))
);

INSERT INTO playit_account (id, is_self_managed, has_premium, updated_at)
VALUES (1, 0, 0, '1970-01-01T00:00:00Z');

-- One tunnel per server, and that is the primary key: two tunnels to the
-- same server would be two of four ports for nothing.
CREATE TABLE playit_tunnels (
    server_id   TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
    -- playit's UUID. NULL while the creation is running.
    tunnel_id   TEXT UNIQUE,
    -- Always from allocations.is_primary; never from a request. See PLAYIT.md 2.3.
    local_port  INTEGER NOT NULL CHECK (local_port BETWEEN 1024 AND 65535),
    state       TEXT NOT NULL CHECK (state IN ('pending', 'online', 'offline',
                                               'missing', 'failed')),
    -- What players type in, exactly as playit delivers it. A JSON field,
    -- because connect_addresses is a list and we throw none of it away.
    addresses   TEXT NOT NULL DEFAULT '[]',
    detail      TEXT,
    created_at  TEXT NOT NULL,
    checked_at  TEXT
);
```

Why `addresses` as JSON and not as a table of its own: the list has zero to three entries, it is
always replaced as a whole and never queried entry by entry. A table for it would be a join for
nothing. The same consideration as with `server_property_overrides`: there the table exists because
individual keys are changed; here they are not.

### 7.1 `0008_playit_per_user.sql`: one account per user

The number is not 0007: the series is not gapless (0003 is missing, the `_sqlx_migrations` of the
running database contains 1, 2, 4, 5, 6, 7), and sqlx applies in version order and does not require
gaplessness.

* **`playit_accounts`** replaces `playit_account`: `user_id TEXT PRIMARY KEY REFERENCES users(id) ON
  DELETE CASCADE`, otherwise the same columns including **both** `CHECK` pairs. **No seed row**: a
  user without a row has connected nothing, and that is the normal case for almost every account.
  That is why `store::account` is a `fetch_optional` and answers "knows nothing".
* **`playit_tunnels` is rebuilt**, not extended with `ALTER TABLE`: only that way is
  `user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE` possible (SQLite has no way to add
  `NOT NULL` afterwards). Order: `DROP` the trigger, new table, `INSERT … SELECT` with
  `user_id = (SELECT owner_id FROM servers WHERE id = server_id)`, `DROP` the old table, rename,
  `CREATE INDEX playit_tunnels_user`. The trigger goes first so that no detour via a `DELETE` is
  even conceivable.
* **`playit_released` gets a `user_id`** and **no** foreign key: without the user, a leftover debt
  after an account is deleted can no longer be settled, because the key it would have to be given
  back with is exactly one. The trigger is recreated with `OLD.user_id`.
* **`playit_account` stays**, untouched. An adoption that goes wrong has to remain readable
  afterwards; only a later migration may drop the table. The manual way back is then two commands:
  move the file back, delete the `playit_accounts` row.

### 7.2 Adopting the old panel-wide account

`playit/legacy.rs`, called from `Playit::start()` **before anything else**, once, recognizable by the
key file no longer lying in the old place:

1. Is `<data_dir>/playit/secret` present as a **file**? Otherwise: do nothing, log nothing.
2. **Measure first:** `SELECT min(s.owner_id) FROM playit_tunnels t JOIN servers s ON s.id =
   t.server_id HAVING count(DISTINCT s.owner_id) = 1`. If all tunnels hang off servers of **one**
   user and that user is an administrator, the account is theirs: every one of those tunnels was
   made with exactly this key, and they are the one who would be facing "no account connected" after
   the update. What is measured is `servers.owner_id`, not `playit_tunnels.user_id`: the rows are
   written before the file (6.), so a run that failed during the move has already left its own answer
   there.
3. **Otherwise the set rule:** the oldest administrator, `SELECT id FROM users WHERE role='admin'
   ORDER BY created_at, id LIMIT 1`. The tiebreaker on `id` is necessary, not decoration:
   `Timestamp` has second resolution (`model.rs`, `replace_nanosecond(0)`), two admins can carry the
   same `created_at`; the ULID restores the creation order. Without an admin: do nothing, one
   `warn!` line. Three shapes measure nothing: no tunnel (a key that was connected and never used),
   tunnels on servers of several users (an admin who made an address for somebody else leaves the
   same trace) and a single owner without the admin role (a panel-wide key does not go to somebody
   who would never have been allowed to connect it). The tunnel of a deleted server is no longer a
   row here but a debt in `playit_released`, without a server, so without an owner; it measures
   nothing and contradicts nothing.
4. Copy the `playit_account` row in full to `playit_accounts(user_id = <admin>)`, claim columns
   included: a claim that happened to be running during the update then does not cost fifteen
   minutes.
5. **Somebody else's tunnels are neither deleted nor given back.**
   `playit_tunnels.user_id = <admin>` for all of them, because that is the truth: they sit on that
   account and cost its ports. The route via `playit_released` would be the other one, and it would
   violate "no data loss without notice": within 30 seconds the reconcile loop would delete a
   stranger's working address at playit. This way it stays, keeps being shown on the server page, and
   whoever carries it can give it back themselves (8.9).
6. `rename` to `<data_dir>/playit/<admin>/secret` (directory 0700, file keeps 0600). **Move, not
   copy-and-delete**, and no `unlink` on any error path: if the move fails, the file stays and the
   next start tries again. That is why the rows come *before* the file. If the admin already has a
   key of their own, everything stays put: overwriting an account of theirs would be a loss without
   notice.
7. Remove the old `<data_dir>/playit/playitd.sock` (3.5).
8. The notice: one `info!` line with the user, the count and the rule, and on their account page as
   in the overview the derived line **"x of 4 · N of them for other users' servers"**
   (`ports.for_others`).
   No `adopted_at` column and nothing to click away: the line is permanently true as long as the
   state lasts, and disappears by itself when it ends.

**Measuring comes before setting a rule**: which admin connected the account back then is written
down nowhere, but who uses it is written in the tunnels. Where there is nothing to measure, the set
rule stands — and it is reversible, because the file is moved.

---

## 8. The design: the endpoints

Eleven, and `8.n` is the same as `CONTRACT.md` `18.n`: six for the signed-in user's **own** account
(8.1–8.6), three at the server (8.7–8.9), two for the panel admin over other people's accounts
(8.10, 8.11). Gone are `POST/GET/DELETE /admin/playit/claim`, `POST /admin/playit/agent/restart` and
`DELETE /admin/playit` without a user: an admin connects an account for nobody, because the
confirmation happens in the account holder's browser.

Style as in `docs/api/CONTRACT.md`: everything under `/api/v1/`, JSON in `snake_case`, session cookie
`craft_session`, errors as `{ "error": "<code>", "message": "<text>" }` (section 1.7 of the
contract). For **every** endpoint `401 unauthenticated`, `403 forbidden`, `404 server_not_found` on
server-scoped paths and `500 internal` apply without being repeated, and for every modifying one
`403 csrf_origin_mismatch`.

**The switch from 12.10 applies.** playit is an outbound service; with `external_services_enabled`
off, every modifying endpoint of this section answers `409 external_services_disabled`, and the
agent is not started.

### 8.0 New error codes

Eight, all prefixed with `playit_` so that the catalog in 1.7 shows at a glance where they come
from:

| Status | Code | Meaning |
|---|---|---|
| 409 | `playit_not_configured` | This account has no key; go through 8.2 first. With 8.8 it is the **owner's** account |
| 409 | `playit_already_claimed` | This user already has a key; 8.5 first |
| 404 | `playit_claim_not_found` | No claim in progress |
| 409 | `playit_tunnel_exists` | This server already has a tunnel |
| 404 | `playit_tunnel_not_found` | This server has none |
| 409 | `playit_has_tunnels` | Disconnecting although tunnels are still on record (8.5) |
| 409 | `playit_port_limit` | Their playit account has no free port left; the sentence names the numbers |
| 409 | `playit_no_primary_port` | The server has no primary port allocation a tunnel could point at |

`playit_tunnel_exists` occurs in a second place that does not live in this section:
**`PUT /servers/:id/allocations/:port/primary` (contract 9.10) refuses with it as long as this server
has a public address.** Where a tunnel points lives on playit's side (2.3), and none of our six calls
can change it; swapping the primary port would leave the hole from the internet on a number this
server no longer holds, and the pool gives a freed port to the next server that asks for one. Give
the address back first (8.9), then swap.

Reused are `502 upstream_unavailable` (playit does not answer, or answers unintelligibly; `message`
names playit and the text from there), `409 external_services_disabled`,
`429 upstream_rate_limited` (429 from playit) and `409 server_busy`.

### 8.1 `GET /api/v1/playit`

Permission: **signed in**, and it is always your own state. Response `200` `PlayitStatus`.

```ts
export type PlayitAgentState = 'absent' | 'starting' | 'running' | 'failed'
export type PlayitBinaryState = 'absent' | 'fetching' | 'ready' | 'failed'

export interface PlayitStatus {
	/** A key is present. Says nothing about whether it is still valid. */
	configured: boolean
	/** playit's UUID for the agent, not a ULID. null until the first rundata. */
	agent_id: string | null
	account_status: 'guest' | 'email_not_verified' | 'verified' | null
	/** From AgentPermissions. false means: this design does not hold (1.4, 2.2 b). */
	is_self_managed: boolean
	has_premium: boolean
	agent: { state: PlayitAgentState; version: string | null; detail: string | null }
	binary: { state: PlayitBinaryState; version: string | null; arch: string; detail: string | null }
	/** Count of *their* tunnels against *their* port limit. The limit is derived,
	 *  not reported by playit: 4 without, 16 with premium (PLAYIT.md 5.1).
	 *  for_others: of those, the ones on other people's servers, only after the adoption (7.2). */
	ports: { used: number; limit: number; for_others: number }
	claim: PlayitClaim | null
	last_error: string | null
	checked_at: Rfc3339 | null
}
```

The endpoint calls **nothing** at playit; it reads the row. Otherwise the account page would hang on
the reachability of somebody else's service. It never carries the key and never a field the key
could sit in — an assertion in `api/playit.rs` nails the field list down.

Errors: none beyond the general ones.

### 8.2 `POST /api/v1/playit/claim`

Permission: **signed in**, for your own account. No body. Response `201` `PlayitClaim`.

```ts
export interface PlayitClaim {
	/** Ten hex characters (PLAYIT.md 1.2). */
	code: string
	/** Always https://playit.gg/claim/<code> — append nothing (1.3). */
	url: string
	state: 'waiting_for_visit' | 'waiting_for_user' | 'accepted' | 'rejected'
	started_at: Rfc3339
	/** Our own deadline, 15 minutes after started_at (1.6). */
	expires_at: Rfc3339
}
```

Generates the code, calls `/claim/setup` **once** and starts the background loop (9.2). The response
is deliberately `201` and not `202`: the URL is there immediately and in full, it is the result.

Errors: `409 playit_already_claimed`, `409 external_services_disabled`,
`502 upstream_unavailable`, `429 upstream_rate_limited`.

**N claim loops are N × 0.5 calls per second to `api.playit.gg` for up to 15 minutes** (450 per
claim), and playit's rate limits are not measured (1.8). That is why `Playit` holds a `Semaphore`
with **four** permits; `poll_claim` takes one before the first `setup` and keeps it to the end.
Whoever gets none keeps the row and the URL — the response is complete at the moment it is handed
over — and starts polling when one becomes free. The deadline keeps running from `started_at`. No
API change, no new error code.

### 8.3 `GET /api/v1/playit/claim`

Permission: **signed in**, your own claim. Response `200` `PlayitClaim`,
`404 playit_claim_not_found`.

Reads the row the background loop keeps updating. No outside call. The interface polls every two
seconds as long as the dialog is open. When `state` switches to `accepted`, the key is already
written; `GET /playit` shows it.

### 8.4 `DELETE /api/v1/playit/claim`

Permission: **signed in**, your own claim. Response `204`. Aborts the open claim and empties the
three `claim_*` columns. Errors: `404 playit_claim_not_found`.

### 8.5 `DELETE /api/v1/playit`

Permission: **signed in**, your own account. The decision about the tunnels sits in the query so
that no body hangs off a `DELETE`, the same shape as 12.6 of the contract:

| Call | Effect |
|---|---|
| without a parameter | `409 playit_has_tunnels` as soon as one tunnel is still on record |
| `?tunnels=delete` | delete all tunnels at playit first, then disconnect |
| `?tunnels=keep` | leave the tunnels at playit, only forget them here |

Response `204`. Stops **their** agent, deletes `<data_dir>/playit/<user_id>/secret`, removes their
rows from `playit_tunnels` and their row from `playit_accounts`. Other people's accounts, other
people's key files and other people's tunnels are not touched; there is a test for that. The binary
stays in the shared cache.

`?tunnels=keep` is the honest way out when playit is unreachable: the tunnels then keep occupying
ports on the account, but you get out of the dead end. The response text says so.

Errors: `409 playit_has_tunnels`, `502 upstream_unavailable` (only with `?tunnels=delete`).

### 8.6 `POST /api/v1/playit/agent/restart`

Permission: **signed in**, your own agent. No body. Response `202` `PlayitStatus`.

Resets the backoff counter and lets `tune_agent` decide again: if there is a tunnel and the switch is
on, the daemon starts up again. That is the one button for "it's stuck", and it replaces a panel
restart.

Errors: `409 playit_not_configured`, `409 external_services_disabled`.

### 8.7 `GET /api/v1/servers/:id/playit`

Permission: **`BASE_READ`**. Response `200` `ServerTunnel`.

```ts
export interface PlayitAddress {
	/** What players type in. Delivered by playit, not built by us (2.4). */
	address: string
	/** 'auto' is the name playit assigns, 'ip4'/'ip6' the bare address,
	 *  'domain' a domain of the account's own. Verbatim from ConnectAddress. */
	kind: 'auto' | 'ip4' | 'ip6' | 'addr4' | 'addr6' | 'domain'
}

export interface ServerTunnel {
	state: 'none' | 'pending' | 'online' | 'offline' | 'missing' | 'failed'
	/** In playit's order. Empty while state !== 'online'. */
	addresses: PlayitAddress[]
	local_port: number | null
	/** A plain sentence when state is neither 'none' nor 'online'. */
	detail: string | null
	created_at: Rfc3339 | null
	checked_at: Rfc3339 | null
}
```

`state: "none"` instead of `404`, so the page knows a single shape and no error path is needed for
the normal case. A viewer may read this: they should be able to pass the address on to their friends
without being allowed to touch the server. The endpoint carries **no** port counts and must not get
any — a viewer has nothing to learn about the owner's account.

### 8.8 `POST /api/v1/servers/:id/playit`

Permission: **owner of the server or panel admin.** No body. Response `202` `ServerTunnel` with
`state: "pending"`.

**The tunnel is always created on the owner's account** (`playit.of(owner_id)`), even when a panel
admin presses the button: the address outlives their click, and the ports it costs are the ones the
owner sees on their account page. If the owner has no key, the endpoint answers
`409 playit_not_configured` with the sentence "the owner of this server has not connected a
playit.gg account" — same code, different sentence.

**Why not `ADVANCED`, although the endpoint lives next to the port allocations.** An editor may
write files and restart the server; all of that acts inside what the owner has already opened.
Creating a tunnel makes the server reachable for **the whole internet**, and that is an owner's
decision. The shape is familiar: `DELETE /servers/:id` (4.5) is owner or panel admin for the same
reason and not `ADVANCED`. The second half of the old reasoning — "one of four panel-wide ports, so
somebody else's port" — has fallen away with the account per user (5.1); the rule stays, its
reasoning is weaker.

The flow in the background:

1. Read `local_port` from `allocations.is_primary = 1`. **From nowhere else.**
2. Write the row with `state: "pending"`, answer `202`.
3. `POST /v1/tunnels/create` with `tunnel_type: "minecraft-java"`, `origin.agent`, region `global`,
   `agent_config.fields = [{local_ip: "127.0.0.1"}, {local_port: "<port>"}]`, `name` = the server
   name cut to 32 ASCII characters.
4. `POST /v1/tunnels/list` every two seconds, for at most 60 seconds, until the new tunnel ID has
   addresses → `state: "online"`. After that `state: "failed"` with the last known reason.

The reason for 60 instead of the plugin's ten seconds: the plugin has a chat channel where a second
message can follow. We have a row in the database, and it is supposed to be right.

**Their agent is started in step 2, not in step 4**, that is, before the creation task sets off, not
after. From step 2 on there is a row, so `tune_agent` finds a user with a key and a tunnel and brings
their daemon up (and their reconcile loop with it, if this is their first address since the panel
started). The other way round, step 4 would run its full sixty seconds against a daemon that does not
exist yet, and would then write "offline" with a reason that reads like a failure
(`playit/connection.rs:530-534`). From the same angle, for step 4: "seen, but not finished yet" is
`offline` and **not** `failed`: the reconcile loop sorts it out by itself, and `failed` would offer
a button that deletes a tunnel which is only slow (`playit/connection.rs:673-675`).

Errors: `409 playit_not_configured`, `409 playit_tunnel_exists`, `409 playit_port_limit`,
`409 external_services_disabled`, `409 server_busy`, `502 upstream_unavailable`,
`429 upstream_rate_limited`.

### 8.9 `DELETE /api/v1/servers/:id/playit`

Permission: **owner of the server or panel admin.** Response `204`.

Deletes at playit first (`POST /tunnels/delete`), then here, and does so through the account that
**carries** the tunnel (`tunnel.user_id`), that is the owner, except for an address from the old
panel-wide account (7.2). **In that order**: a row we forget while the tunnel stays over there costs
one of their ports permanently and can no longer be reached through our interface. If playit does not
answer, `502 upstream_unavailable` and **nothing** changes.

The one exception is deleting the server (4.5) and disconnecting with `?tunnels=keep` (8.5): there
the row goes in every case, because otherwise somebody else's service could prevent the deletion of
a server of your own.

Errors: `404 playit_tunnel_not_found`, `502 upstream_unavailable`.

### 8.10 `GET /api/v1/admin/playit`

Permission: **panel admin**. Response `200` `PlayitOverview[]`, one row per user with a row in
`playit_accounts`, sorted by user name.

Per row: `user_id`, `username`, `configured`, `account_status`, `is_self_managed`, `has_premium`,
`agent`, `ports {used, limit, for_others}`, `last_error`, `checked_at`. **No `claim`** — a code is a
way in to somebody else's account — and no field the key could sit in. It is read from the database
with `LEFT JOIN users`; `agent` and `configured` are what this panel process knows, otherwise
`absent`.

### 8.11 `DELETE /api/v1/admin/playit/{user}?tunnels=delete|keep`

Permission: **panel admin**. Response `204`, errors as in 8.5 plus `404 user_not_found`.

This stays, because the port debt is real: without it, a user who has lost their password or is
locked out holds four ports and a running agent forever. It writes a `warn!` line naming the actor
and the target — **an audit entry in the sense of the audit log is not possible**, `audit::record`
and `record_by` demand a `server_id`, and a panel-wide log does not exist. That is a gap, not an
invention; it belongs named, not papered over with a bogus entry on an arbitrary server.

### 8.12 What explicitly does not exist

* **No endpoint that takes a port.** Reasoning in 2.3.
* **No `refresh`.** The background loop reconciles every 30 s; a button for it would be a button
  that speeds up nothing the user notices.
* **No endpoint with which an admin connects an account for somebody.** The confirmation happens in
  the browser of the person the account belongs to; such an endpoint would be a lie about what
  happens there.
* **No operation in the sense of section 5 of the contract.** Creation takes seconds and has no
  phases; the claim would not even have a `server_id` under 17.14. The intermediate state `pending`
  in a small response of its own is cheaper than a new kind of operation with its locking rules,
  cancellation and retry.
* **No WebSocket.** The socket is one per server (contract 13) and carries `server`, `state`,
  `stats`. A tunnel state does not belong in there, and a second socket is out of the question.

---

## 9. The design: what runs in the panel

A new module `crate::playit`, built like `crate::loaders`: an `http` layer with timeouts, one file
per subarea, parsers against real test files.

```
crates/craftpanel/src/playit/
├── mod.rs        Playit — the broker: one Connection per user, and what belongs to nobody
├── connection.rs one account: its key, its agent, its four ports, its two loops
├── legacy.rs     adopting the old panel-wide account (7.2)
├── http.rs       Agent-Key, timeouts, the three-part response format from 1.7
├── claim.rs      generate a code, setup, exchange, the state machine — stateless, no user
├── agent.rs      Binary (shared, with a lock) and Agent (one per user)
├── tunnels.rs    create, delete, reconcile
├── store.rs      the three tables, every count keyed by user
└── testdata/     the eleven real responses this document rests on
```

`claim.rs` is **stateless and knows no user**: `setup`, `exchange`, `after_setup`, `after_exchange`
take a `&Http` and a code. That is deliberate: `after_exchange` maps `CodeNotFound` → `Step::Wait`,
because playit answers that for a code one second old (1.5, 81 measurements). Turn it into
`Step::Stop` and you kill every claim before the browser is even open; the two tests that hold this
are among the few that have to stay character-for-character the same.

### 9.1 The six routes we speak

No more than that. Every further one is explicitly ruled out in section 11.

| Route | what for | Response type |
|---|---|---|
| `POST /claim/setup` | start a claim and poll it | `ClaimSetupResponse` |
| `POST /claim/exchange` | fetch the key, **only after `UserAccepted`** | `AgentSecretKey` |
| `POST /v1/agents/rundata` | agent ID, account state, permissions, notices | `AgentRunDataV1` |
| `POST /v1/tunnels/create` · `/v1/tunnels/list` · `/tunnels/delete` | the tunnels | `ObjectId` · `AccountTunnelsV1` · `()` |

For deleting there is nothing in the v1 series; `/tunnels/delete` is the older route and present in
both official clients (`agent:api.rs:138-144`, `plugin:ApiClient.java:110-112`). Body
`{"tunnel_id": "<uuid>"}`.

### 9.2 Three background loops — per connected user

| Loop | runs when | Interval | does |
|---|---|---|---|
| **claim** | only while their claim is open, at most 15 min from `started_at`; at most four in the whole panel at the same time (8.2) | 2 s | `/claim/setup`; on `UserAccepted` one `/claim/exchange`, write the key file, one `rundata`, agent according to `tune_agent` |
| **agent** | as long as they have a key **and** a tunnel and the switch is on (3.1) | event-driven | keep `playitd` as a child, backoff 1 s → 60 s, quiet after five failed attempts in a row — and then no silent revival by `tune_agent` |
| **reconcile** | as long as their key is present | 30 s, staggered | `tune_agent`; then, only if they have a tunnel or an outstanding debt, `rundata` + `tunnels/list` and update their rows |

The reconcile starts at `hash(user_id) % 30 s`, so that twenty users do not fire two HTTPS calls each
in the same second. And it calls nobody when there is nothing to reconcile: a connected user without
an address costs one sleeping task and no outside call. The round keeps running even with the switch
from 12.10 turned off, though: it is the one that notices the switch coming back (`follow_switch`
has been absorbed into `tune_agent`).

Timeouts for every outside call, the same numbers as `loaders::http`: connect 5 s, response 15 s.
The download of the binary 300 s in total. **No call to playit ever sits in a request path a browser
is waiting on**, with one exception, the single `/claim/setup` in 8.2, and that is the call whose
result the user wants to see.

### 9.3 What has to go into `main.rs`

`main.rs` belongs to nobody. These lines are what I need there — no more:

1. **Register the module**, alphabetically between `ops` and `servers`:
   ```rust
   mod playit;
   ```

2. **Build the service**, in `serve()` after `let sources = …` and before `let manager = …`:
   ```rust
   let playit = playit::Playit::new(pool.clone(), Arc::clone(&state.config))?;
   playit.start();
   ```
   `start()` takes itself as an `Arc`, first adopts the old panel-wide account (7.2) and then wakes
   every user with a row in `playit_accounts`: an open claim and, if a key file is there, their
   loops. Anybody it does not concern costs nothing, and the only thing logged is how many accounts
   were picked up.

3. **Mount the router**, in the `Router::new()` chain, after `api::access::router()`:
   ```rust
   .merge(api::playit::router(Arc::clone(&playit)))
   ```

4. **When a server is deleted** `servers::manager::Manager` does **not** need the service: the tunnel
   row hangs off the server (`ON DELETE CASCADE`), the trigger writes the tunnel ID with its user to
   `playit_released`, and that user's reconcile loop gives it back. That way no outside service holds
   up the deletion of a server of your own.

5. **When an account is deleted** `api::admin` does need the service, and that is the one existing
   signature that changes:
   ```rust
   .merge(api::with_live(live.clone(), disks.clone(), Arc::clone(&playit)))
   ```
   Before `DELETE FROM users`, `dispose_of` gives back every address of that account (fallback: the
   debt stays on record under their name), stops their agent and removes
   `<data_dir>/playit/<user>`. Before `UPDATE servers SET owner_id`, `hand_over` gives back the
   address of every server handed over: it hangs off the old owner's account, and only their key can
   give it back.

Plus one line in `crates/craftpanel/src/api/mod.rs` — that file is not mine either —
`pub mod playit;` between `pub mod files;` and `pub mod servers;`.

---

## 10. The design: the interface

Three places, no fourth. All three are in files of our own; **nothing in `vendor/modrinth/` is
changed.**

### 10.1 Your own account page: the "Public addresses" section

`web/src/pages/account/Playit.vue`, mounted as the last card in
`web/src/pages/account/Account.vue` (route `account`, menu entry "Your account"). The computation
sits next to it in `web/src/pages/account/playit.ts` so that it can be tested without jsdom.

Not connected: a paragraph on what this is, and a button "Connect to playit.gg". The button opens a
dialog with the URL, a copy button and the line "Waiting for the confirmation in the browser…",
which polls 8.3 every two seconds. Confirmed: dialog closed, the section shows the agent state, the
account state, `3 of 4 ports in use` and "Disconnect".

Three states, because they need three sentences: `unconnected`, `quiet` (connected, no server of
theirs has an address → "No server of yours has a public address, so the tunnel service is not
running"; not an error, see 3.1) and `live`.

With `account_status: "guest"` a sentence next to it says that a guest account without an e-mail
address can be lost. With `is_self_managed: false` **and** after the first reconcile, it says there
that this agent may not manage tunnels and has to be connected again; that is the visible outcome
of the caveat from 2.2 (b); before `checked_at` the value is only the row's initial value and no
alarm.

The panel-wide switch from 12.10 has to stay visible here: a user can connect their account and
still get nothing. They learn it from the refusal `409 external_services_disabled`: they are not
allowed to read the panel settings, so they do not get the button leading there either.

### 10.1a Admin → "Public addresses (all users)": the overview

`web/src/pages/admin/Playit.vue` (route `admin-playit`, menu entry for admins only) is a table over
8.10: user, account state, tunnel service, `used/limit` including "N for other users' servers", last
confirmation, last error, and per row a "Disconnect" over 8.11. No connect card, no claim dialog, no
claim code. Below `md` the same thing stands one item under another instead of behind a sideways
scroll.

### 10.2 Server: the "Public address" block

`web/src/components/PlayitAddress.vue`. It was built as a component of its own and today sits in
`web/src/pages/servers/Overview.vue` right above the console, where people look for the address,
and not three clicks deep under the settings. The design said `servers/settings/Network.vue`; that is
the one place where the build departed from the design, and the component is the same in both
places.

| State | what it says |
|---|---|
| the owner has no account connected | a sentence on what this is. For the **owner**, plus the button "Set up playit.gg" leading to their account page (10.1) — they can do it themselves. Somebody else looking only sees the sentence: "the owner of this server has not connected a playit.gg account" |
| set up, no tunnel | button "Request a public address" (owner and panel admin only, otherwise grayed out with an explanation) |
| `pending` | a spinner, "The address is being set up…" |
| `online` | **playit's first** address as a `CopyCode` line, that is the one to pass on; the rest collapsed next to it, with the reason (see below) |
| `offline`, `missing`, `failed` | `detail` as a sentence, button "Create again" |

**Measured on a running tunnel, and it changed this table.** For the tunnel of a real server playit
delivered four addresses:

```
mauritania-nice.tun.ply.gg      auto
231.ip.gl.ply.gg:15878          auto
147.185.221.231:15878           addr4
[2602:fbaf:0:1::e7]:15878       addr6
```

Of those four, **one** carries. Minecraft sends the typed address along in the handshake, and
playit's edge only forwards the name of the tunnel: same IP, same port, only the name swapped:

```
147.185.221.231:15878, handshake "147.185.221.231"            → Connection reset by peer
147.185.221.231:15878, handshake "mauritania-nice.tun.ply.gg" → Paper 1.21.4, 0/20 players
```

That made the first version of this line a trap: four equal copy buttons, three of them without a
connection. And the IP line is **no** way out of "Unknown host": playit's own answer to that is a
different DNS server on the player's machine
(`playit.gg/support/minecraft-java-unknown-host`), because an IP the edge rejects helps nobody
resolve anything. So what gets passed on is playit's first address, the way their own plugin does it
(2.4). `scripts/reachable-publicly.py` queries an address the way the game does, and is the way
to check this.

### 10.3 The address where people look for it

The wish was "they get an IP right away". In a subpage of the settings it is half delivered. It
belongs in the server header, where an address already stands today, checked in
`web/src/layouts/ServerFrame.vue`:

```ts
// ServerFrame.vue:277-281 — the address as a CopyCode badge next to the state,
// on every page of every server
const address = computed(() => {
	const { ip, port } = server.value.net
	return `${ip ?? location.hostname}:${port}`
})
```

**Exactly this one computation gets extended:** if a tunnel with `state === "online"` is present, its
first address stands there; otherwise everything stays as it is. That is one line in a file of our
own and the only one needed for the whole wish.

**What explicitly is not done: overwriting `Server.net`**, neither in the backend nor in
`web/src/providers/server-context.ts`. Two measured reasons:

* In the contract `ServerNet.ip` is `admin_settings.public_address` (12.10) and `net.port` the
  **local** port. Our own `Network.vue` builds the "Primary" line of the allocation table from them
  (`Network.vue:282`, `:318`). Bend `net` and you show the playit port there as the local port, and
  the "Make primary" button next to it works with the real ones.
* It would also gain nothing. Counted: `net.ip`/`net.port` is read in `vendor/modrinth/` only by
  `layouts/shared/server-settings/pages/network.vue`, the page we have replaced with our own. No
  component we actually show depends on it. Modrinth's `ServerSubdomainLabel.vue:18` hard-appends
  `.modrinth.gg` and therefore stays invisible with `domain: ""` (contract 14, `ServerNet`).

**Where the header gets the tunnel from.** `ServerFrame.vue:119-120` provides both contexts:

```ts
const page = provideServerContext({ server: props.server, socket })
provideServerPage({ ...page, serverId: props.server.id, socket })
```

A `playit: Ref<ServerTunnel>` is added there, filled from `GET …/playit` and refetched every 30
seconds, and `ServerPage` in `web/src/composables/server-page.ts` gets the field. That way the header
(10.3) and the network block (10.2) read **the same** source, and there is no second fetch path that
could drift apart.

---

## 11. What is not built

playit can do far more than we need. Without this list it creeps in during the build anyway.

**Of playit's features:**

| Not | Why |
|---|---|
| Regional tunnels | Premium; on a free account nothing but an error message (5.2). We set `global` and leave it |
| Dedicated IP addresses (`dedicated-ip`) | Premium |
| `.playit.plus` domains and external domains | Premium; and anybody who has a domain of their own can also forward a port |
| Firewalls per tunnel (`/tunnels/firewall/assign`) | Premium for more than the basics, and a whole management surface for an access list Minecraft already has (`whitelist`) |
| Rate limits per tunnel (`/tunnels/ratelimit`) | the same |
| Proxy protocol (`proxy_protocol`) | demands a change in `paper-global.yml` or `velocity.toml` and breaks the server when the two sides do not match. It would gain real player IPs in the log — not worth it as long as nobody asks for it |
| HTTPS tunnels | for web servers, not for Minecraft |
| UDP and Bedrock tunnels (`minecraft-bedrock`, Geyser on 19132) | Bedrock is explicitly out in `docs/PLAN.md:432`. Technically it would be a different `tunnel_type` and nothing else — if it is ever wanted, it is a field, not a construction site |
| Several tunnels per server, port ranges (`port_count > 1`) | four ports in the whole panel (5.1). The second tunnel of one server would be the missing first one of another |
| Account management: rename the agent, set routing, guest sign-up link, reset the password, shop, billing | That is playit's interface. We do not rebuild it, just as we did not rebuild Modrinth's billing (`docs/PLAN.md:93`) |
| `playit-cli`, its IPC socket, its TUI, `playit attach`, `playit status` | We only need the daemon (3.2) |
| playit's systemd unit, their `apt` package, their Docker image | The agent is our child process (3.4) |

**On our side:**

| Not | Why |
|---|---|
| ~~One playit account per panel user~~ | **This line is reversed: that is exactly how it is built now.** The reason: the panel provides no account for other people's servers, and one admin's four ports were the ceiling for everybody. What was right about the old reasoning: a friend who only **plays** still clicks nothing — they type in an address. An account is connected only by those who want to publish **their own** servers. What it kept quiet about: the admin was managing other people's access, and their account carried other people's ports |
| A panel-wide account the admin connects for everybody | Four ports for the whole panel, somebody else's account in somebody else's hands, and an admin who cannot give a confirmation in another person's browser. Adopting the old account is in 7.2 |
| Automatic tunnels for every new server | Four ports. The fifth server would get an error message in the create wizard, and the wizard is Modrinth's unchanged flow. The tunnel is requested, not included |
| The tunnel in `POST /servers` | the same, and it would make creation depend on somebody else's service |
| Automatic updates of the agent | The checksum lives in the source code (4.2). A new version is a panel update |
| A system account of its own for the agent | 3.3 |
| A supervisor for the agent | 3.4 |
| An operation, a WebSocket, a `refresh` | 8.10 |
| An endpoint that takes a port | 2.3 — and that is the only line of this table where a violation is a security hole, not scope |

---

## 12. What stays open

Six points. The first two the first builder has to measure before writing code that depends on them;
the last two came in with "one account per user".

1. **The body shape of `/v1/tunnels/create`.** Two official clients, two pairs of field names
   (2.2 a). First real call with a key, store the response as a test file, remove the losing shape
   from the code.
2. **Whether `Agent-Key` is allowed on `/v1/tunnels/create`** (2.2 b). All the evidence says yes; an
   `AgentNotSelfManaged` would be the counter-proof and would overturn the design.
3. **The upper bound for a claim code.** Measured: at least forty minutes, 81 measurements, no
   `CodeExpired` (1.6). Our 15-minute deadline is therefore certainly shorter than playit's; it is
   a rule we set for our background loop, not a measurement of theirs. If a code does expire against
   expectations, the loop notices at the `CodeExpired` and generates a new one; the case is handled,
   the number is just not known. **No reason to hold up the build.**
4. **What a Minecraft Java address looks like exactly** — a name without a port over SRV, a name
   with a port, or both side by side. That is why `addresses` is a list and why nothing is assembled
   (2.4). As soon as a real tunnel exists, its `tunnels/list` response belongs here as a test file;
   only that says which of the six `ConnectAddress` shapes actually arrives and what the server
   header from 10.3 shows.

5. **Whether playit allows several accounts with one agent each without complaint.** Unchecked,
   because there was no second account at hand. All the evidence says yes: the premium pitch counts
   "more … agents" as an advantage *within* one account, and one account with one agent is the normal
   case for their own plugin. The counter-proof would be a refusal at the second `claim/exchange`.
6. **playit's rate limits** (1.8). With N claim loops and N reconcile loops the question has become
   more expensive than before. Hence four permits for the claim (8.2) and a stagger for the reconcile
   (9.2), both rules we set, not measurements.

And a seventh that needs no measurement but a decision: **we are hanging the reachability of
friends' servers on a free service from a third party.** If playit goes down, the servers are only
reachable locally. That is the trade, and it is a good one — the alternative is port forwarding,
which many connections no longer offer at all. It just belongs written down somewhere, and here it
is.

---

## Appendix: the stored test files

All of them in `crates/craftpanel/src/playit/testdata/`. The eleven in the first table were really
fetched on 2026-08-13; the three in the second were **not**, and they say so in their names.

| File | Origin |
|---|---|
| `claim_setup_waiting_for_visit.json` | `POST /claim/setup` with a fresh code |
| `claim_exchange_not_ready.json` | `POST /claim/exchange` before the confirmation — the case from 1.5 |
| `claim_setup_invalid_code.json` | `POST /claim/setup`, code two characters |
| `claim_setup_version_too_long.json` | `POST /claim/setup`, `version` 300 characters |
| `error_auth_required.json` | `POST /tunnels/list` without `Authorization` |
| `error_invalid_agent_key.json` | `POST /v1/tunnels/list` with `Agent-Key deadbeef` |
| `error_path_not_found.json` | `POST /nope/nothing` — the case with `message` as an **object** |
| `error_validation.json` | `POST /claim/setup` with an empty body |
| `info_pops.json` | `POST /info/pops`, the 22 locations |
| `github_release_v1_0_10.json` | release API, the four Linux assets with `digest` |
| `agent_schema_release_v1_0_10.json` | `agent:agent-schema-release.json` — `allow_self_managed` and the field names from 2.3 |

Ten of them are responses from `api.playit.gg`; `agent_schema_release_v1_0_10.json` is the unchanged
file from the agent repository and lives here because it documents the field names from 2.3 and
`allow_self_managed` (checked: character-for-character identical with the file at `v1.0.10`).

Three more carry `_client_types` in their names, and the suffix is the warning:

| File | Origin |
|---|---|
| `tunnels_list_client_types.json` | **not measured** — built from the types in `agent:api.rs` at `v1.0.10` |
| `agents_rundata_client_types.json` | the same |
| `tunnels_create_request_client_types.json` | the same, in the other direction: the body their own client would write |

We have no account, so there is no real response for these three. What they hold on to are field
names and labels — exactly what a parser fails on — and they are checked line by line against
`agent:packages/api_client/src/api.rs` at `v1.0.10`: `ConnectAddress` with `tag = "type", content =
"value"` and six cases (`:671-686`), `PublicAllocation` with `content = "details"` (`:619-625`),
`AccountTunnelOfflineReason` without a rename (`:415-423`), `AgentPermissions` (`:1055-1059`),
`ReqTunnelsCreateV1` (`:751-759`). What they do **not** hold on to is open question 4 from section
12: which of the six address forms a real Minecraft Java tunnel actually delivers.

**The two tests that prove something**, both against real excerpts, both with a mutation check to
demonstrate:

1. Over `error_path_not_found.json`: with `path-not-found`, `message` is an **object**, otherwise a
   string (1.7). Narrow the parser to `message: String` and the test goes red; open it up again and
   it goes green.
2. Over `claim_exchange_not_ready.json`: `CodeNotFound` from `/claim/exchange` is **no** reason to
   abort while the claim is running (1.5). Treat it as an abort and the claim ends before the user
   has even opened the browser — and that is exactly what the test has to turn red on.
