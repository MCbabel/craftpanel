-- What 4.6 needs and 0002 has nowhere to put: the power state of a server has to
-- outlive the panel process. Without it the reconciliation at start-up (5.12) has
-- nothing to compare the attached supervisors against, and a server that crashed
-- while the panel was being updated would come back as merely "stopped".

ALTER TABLE servers ADD COLUMN power_state TEXT NOT NULL DEFAULT 'stopped'
                    CHECK (power_state IN ('stopped', 'starting', 'running',
                                           'stopping', 'crashed'));

-- 13.4: `uptime_seconds` counts from here. In the row and not in memory, so a
-- panel that was restarted under a running server does not report an uptime of
-- zero for a server that has been up for a week.
ALTER TABLE servers ADD COLUMN running_since TEXT;

-- 13.4, the two fields of `state` that describe how the last run ended. The
-- out-of-memory mark is a guess and the contract says so: the cgroup belongs to
-- the user, not to the server.
ALTER TABLE servers ADD COLUMN exit_code INTEGER;
ALTER TABLE servers ADD COLUMN oom_killed INTEGER NOT NULL DEFAULT 0
                    CHECK (oom_killed IN (0, 1));

-- 4.1 lists the servers of one owner over and over, and 4.2 adds up their
-- `-Xmx` before it lets a new one through.
CREATE INDEX servers_owner_memory ON servers(owner_id, status);
