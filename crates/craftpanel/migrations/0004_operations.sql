-- Two columns section 5 needs that 0002 has nowhere to put.

-- 5.4: an ongoing run is *asked* to stop, it does not stop on the spot. The
-- caller gets the operation back still in `ongoing`; the worker reads this on its
-- next step. In a column and not in memory, so any worker can see it with the
-- query it already makes.
ALTER TABLE operations ADD COLUMN cancel_requested INTEGER NOT NULL DEFAULT 0
                       CHECK (cancel_requested IN (0, 1));

-- 5.11 `timeout`, "no progress for ten minutes". Neither created_at nor
-- started_at moves while a run works, so neither can carry that watchdog.
ALTER TABLE operations ADD COLUMN progressed_at TEXT;
