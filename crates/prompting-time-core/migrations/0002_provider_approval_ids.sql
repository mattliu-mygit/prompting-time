ALTER TABLE approvals ADD COLUMN provider_request_id TEXT;

CREATE UNIQUE INDEX idx_approvals_provider_request
ON approvals(run_id, provider_request_id)
WHERE provider_request_id IS NOT NULL;
