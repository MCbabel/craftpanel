-- Ein fünftes Limit je Konto: Plattenplatz über alle Server eines Nutzers
-- zusammen (12.7). Anders als die vier von 0002 erreicht es den Kernel nie —
-- cgroup v2 kennt keinen Plattenplatz — die Prüfung sitzt im Panel, an den
-- Wegen, die durch das Panel führen.
--
-- Die Vorgabe ist absichtlich groß: nach der Migration soll niemand ohne sein
-- Zutun über seiner Grenze liegen und nichts mehr hochladen können.

ALTER TABLE users ADD COLUMN disk_mib INTEGER NOT NULL DEFAULT 51200
                             CHECK (disk_mib >= 1024);
ALTER TABLE panel_settings ADD COLUMN default_disk_mib INTEGER NOT NULL DEFAULT 51200
                             CHECK (default_disk_mib >= 1024);
