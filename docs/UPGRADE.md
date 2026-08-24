# Upgrade from the old name

As of 2026-08-24. **An installation still called `mcpanel` can be moved to CraftPanel by
`install.sh` — the same worlds, the same database, the same uids — and it is moved only when
somebody has said so. Not by itself, and not on Enter.**

`install.sh` recognizes the old installation and first writes down what moving it would do:
**every Minecraft server it is running is told to save and shut down, and stays stopped**, and
**that installation is renamed** — data directory, configuration, both services, every account,
from `mcpanel` to `craftpanel`. Then it asks, and **the default is no**. A run where somebody only
presses Enter leaves the old installation running and untouched and goes on to install CraftPanel
beside it. Moving it takes a typed `y`, or `CRAFTPANEL_UPGRADE=yes` — which is also the only way
in a run without a terminal (`CRAFTPANEL_NONINTERACTIVE=1`, `curl … | bash` in a script), where
the installer says it is leaving `mcpanel` alone and carries on with the ordinary install.

That is deliberate: on one machine the installation people use and a second one for trying things
out may stand side by side (`scripts/acceptance.sh` does exactly that). What is on the other side
of that question is somebody else's servers going down and somebody else's installation losing
its name. One keystroke is not enough to ask for that.

**Once CraftPanel is installed on this machine, the question is not asked at all any more.** Two
accounts of the same purpose cannot both own the files, so the pre-check (section 2, step 1) would
refuse the move; a run that asked anyway would be asking for an answer it could not carry out.
Such a run says in one line that something under the old name is still there, points here, and
goes on with the ordinary update. `CRAFTPANEL_UPGRADE=yes` still forces the attempt, and then the
pre-check names which of the two has to be cleared up first.

---

## 1. What moves

| old | new |
|---|---|
| `/var/lib/mcpanel` | `/var/lib/craftpanel` |
| `/etc/mcpanel/config.toml` | `/etc/craftpanel/config.toml` |
| `mcpanel.service`, `mcpanel-helper.service` together with `*.service.d/` | `craftpanel*.service` together with drop-ins |
| user `mcpanel` (uid 999 on this machine) | `craftpanel`, **the same uid** |
| group `mcpanel` (gid 991) | `craftpanel`, **the same gid** |
| account `mcp-<id>` and the group of the same name | `craft-<id>`, **uid and gid stay** |
| `/sys/fs/cgroup/system.slice/mcpanel-games` | `…/craftpanel-games` |
| `/run/mcpanel` | `/run/craftpanel` |
| `/usr/local/bin/mcpanel{,-helper}` | `/usr/local/bin/craftpanel{,-helper}` |

**uid and gid stay, and that is why every ownership entry on disk stays right.** The upgrade
renames accounts (`usermod -l`, `groupmod -n`), it creates no new ones; not one `chown` runs over
the worlds.

## 2. The order, and why it is this way

1. **Pre-check.** Tools (`usermod`, `groupmod`, `sqlite3` or `python3`), names that are already
   taken, a data directory on a mount of its own. If something fails here, nothing has been
   touched yet.
2. **Fetch the programs.** Before the first stop: a failed download leaves behind an installation
   that keeps running.
3. **Shut the games down cleanly.** Every process in `mcpanel-games` that is not a supervisor
   gets `SIGTERM` on its process group: that is the signal on which the server's shutdown hook
   saves the world. **No `SIGKILL`, ever.** If a server has not gone after `CRAFTPANEL_STOP_GRACE`
   (default 300 s), the upgrade aborts and has moved nothing. The panel is still running while
   this happens, so that every supervisor can hand over its last lines and end by itself.
4. **Stop the services** (`systemctl disable --now`); only after that is anything moved at all.
5. **Clear away the control group.** A cgroup cannot be renamed — the kernel answers `rename(2)`
   on cgroup2 with `EPERM` — so the empty tree is removed. Nothing is lost in the process: at
   startup the panel rewrites the limits of every finished account
   (`auth/users.rs`, `rewrite_limits`), and the helper creates the directory again before the
   next start of a server — under the new name that both units give.
6. **Move the data directory.** An `mv` inside `/var/lib`, which is a rename: no copy, no waiting
   time, no second disk filled up.
7. **Rename the service account and the group.**
8. **Rename every `mcp-` account and its group.** The home directory is derived from the existing
   entry (only `/var/lib/mcpanel` → `/var/lib/craftpanel`), **not** from the account name: the id
   in the name is lower-case, the directory carries it upper-case.
9. **Move the configuration** and replace the names in it. The bind address, the data directory and
   everything else stay as the operator set them. The range the servers' ports come out of is not in
   that file — it is a panel setting and moves with `panel.db`.
10. **Write the units, take the drop-ins along.** The installer writes the two unit files anew,
    they are its own. The drop-ins are not: they belong to the operator and move one by one; in
    file name and content only `MCPANEL_` → `CRAFTPANEL_`, `MCPanel` → `CraftPanel`,
    `mcpanel` → `craftpanel` and `mcp-` → `craft-` are replaced, every setting stays. The old
    unit files then lie under `/var/lib/craftpanel/upgrade-from-mcpanel/`, so you can look up
    what was in them.
11. **Database.** See section 4.
12. **Fix the permissions, start, look.** If `craftpanel.service` stays dead, the run ends with a
    message and the command that shows the log.

Once the panel is running again, everybody who was signed in has to sign in once more: the
session cookie is now called `craft_session`, nobody reads the old `mcp_session` any more, and
whoever's browser still carries the old one lands on the sign-in page. Username and password have
stayed the same — one sign-in, and it goes on as before.

## 3. What the upgrade does not do

- **It does not start the Minecraft servers again.** They were stopped so they would save; if you
  want them running, start them in the panel.
- **Usernames and passwords stay as they are.** The upgrade changes nothing about the accounts in
  the database; only the open sessions end (section 2, last paragraph).
- **A Google Drive folder that already exists keeps its name.** The panel finds it by the stored
  Google id, not by the name (`drive/mod.rs`, `folder`). Only the default for the *next* newly
  created folder is set to `craftpanel-backups`.
- **`/etc/mcpanel` stays**, if anything besides the `config.toml` is still in it.

## 4. What is in the database — and what is not

**No field stores an account name.** The panel computes it from the id every time
(`craftpanel-proto`, `system_username` → `craft-<id>`); `users` holds only `system_uid`. That is
exactly why renaming the accounts on the system is enough — afterwards the database means the
right ones by itself.

In plain text the old name stands in three places, and three statements run for those three:

```sql
UPDATE users
   SET system_error_message = replace(replace(system_error_message, 'mcpanel', 'craftpanel'),
                                      'mcp-', 'craft-')
 WHERE system_error_message LIKE '%mcpanel%' OR system_error_message LIKE '%mcp-%';

UPDATE mail_settings  SET from_name   = 'CraftPanel'          WHERE from_name   = 'mcpanel';
UPDATE drive_settings SET folder_name = 'craftpanel-backups'  WHERE folder_name = 'mcpanel-backups';
```

The first concerns the helper's last complaint about an account (`auth/users.rs`, `provision`) —
the account name and the path stand in it verbatim. The second is the sender name of the mails,
the third the default from `0012_drive.sql`. The latter two are touched only as long as they are
unchanged: if the operator has entered something of his own there, it stays.

## 5. Running it twice does no harm

Every step first asks whether it still has anything to do. After a successful upgrade `install.sh`
finds nothing old any more and offers the usual **update / uninstall** again. If a run breaks off
in the middle, the next one carries on from there: no step is run twice, and none assumes that
the previous one has just run. And if something under the old name is left standing on purpose —
a stray `mcp-` account, a directory nobody cleared away — it is not turned into a question on
every update run: CraftPanel is installed by then, and the last paragraph at the top of this
document says why that settles it.

The upgrade aborts instead of carrying on half finished when: a server does not stop, a service
does not stay down, a target name is already taken, both data directories contain data, `usermod`
refuses, or an SQL statement does not go through. Every time, the message names the next thing to
do.

## 6. Doing it by hand

If the upgrade gets stuck somewhere, this is the same work as single commands. All as `root`, in
order, and every command may be left out if it finds nothing left to do.

```bash
# 1. Stop the servers cleanly (in the panel with "Stop", or by hand):
for p in $(cat /sys/fs/cgroup/system.slice/mcpanel-games/*/cgroup.procs); do
    readlink /proc/$p/exe | grep -q '/mcpanel$' || kill -TERM -"$p"   # never SIGKILL
done

# 2. Stop the services
systemctl disable --now mcpanel.service mcpanel-helper.service

# 3. Control group (only once no process is left in it)
rmdir /sys/fs/cgroup/system.slice/mcpanel-games/user-*
rmdir /sys/fs/cgroup/system.slice/mcpanel-games

# 4. Data
mv -T /var/lib/mcpanel /var/lib/craftpanel

# 5. Service account and group (uid and gid stay)
groupmod -n craftpanel mcpanel
usermod -l craftpanel -d /var/lib/craftpanel -c 'CraftPanel service account' mcpanel

# 6. The accounts of the panel users
getent passwd | awk -F: '/^mcp-/ { print $1, $6 }' | while read -r name home; do
    usermod -l "craft-${name#mcp-}" -d "${home/\/var\/lib\/mcpanel//var/lib/craftpanel}" \
            -c 'craftpanel managed account' "$name"
    getent group "$name" >/dev/null && groupmod -n "craft-${name#mcp-}" "$name"
done

# 7. Configuration
install -d -m 0755 /etc/craftpanel
mv /etc/mcpanel/config.toml /etc/craftpanel/config.toml
sed -i -e 's/MCPANEL_/CRAFTPANEL_/g' -e 's/MCPanel/CraftPanel/g' \
       -e 's/mcpanel/craftpanel/g' -e 's/mcp-/craft-/g' /etc/craftpanel/config.toml
chown root:craftpanel /etc/craftpanel/config.toml && chmod 0640 /etc/craftpanel/config.toml

# 8. Take the drop-ins along, old units gone
cd /etc/systemd/system
for u in mcpanel:craftpanel mcpanel-helper:craftpanel-helper; do
    old="${u%:*}"; new="${u#*:}"
    [ -d "$old.service.d" ] || continue
    install -d "$new.service.d"
    mv "$old.service.d"/*.conf "$new.service.d/"
    sed -i -e 's/MCPANEL_/CRAFTPANEL_/g' -e 's/MCPanel/CraftPanel/g' \
           -e 's/mcpanel/craftpanel/g' "$new.service.d"/*.conf
    rmdir "$old.service.d"
done
rm -f /etc/systemd/system/mcpanel*.service
rm -f /etc/systemd/system/multi-user.target.wants/mcpanel*.service
rm -rf /run/mcpanel
systemctl daemon-reload

# 9. Database: the three statements from section 4
sqlite3 /var/lib/craftpanel/panel.db "…"

# 10. Write the new units, fix the permissions, start — the installer does that:
./install.sh
```

In step 10 the installer writes the two unit files, sets the permissions on the data directory
(`1771` on `/var/lib/craftpanel`, `0751` on `users/`) and starts both services. If
`systemctl status craftpanel` then runs green and the panel shows the old servers, the upgrade is
done.
