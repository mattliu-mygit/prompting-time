ALTER TABLE provider_runs ADD COLUMN dispatch_owner_id TEXT;
ALTER TABLE provider_runs ADD COLUMN dispatch_lease_expires_at INTEGER;

CREATE INDEX idx_provider_runs_dispatch_lease
ON provider_runs(status, dispatch_lease_expires_at)
WHERE dispatch_owner_id IS NOT NULL;
