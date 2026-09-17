ALTER TABLE conversations ADD COLUMN context_budget_json TEXT NOT NULL
DEFAULT '{"kind":"tokens","tokens":300000}'
CHECK (length(CAST(context_budget_json AS BLOB)) <= 256);

-- Historical runs retain native defaults; numbered budgets require new intent.
ALTER TABLE provider_runs ADD COLUMN context_budget_json TEXT NOT NULL
DEFAULT '{"kind":"providerDefault"}'
CHECK (length(CAST(context_budget_json AS BLOB)) <= 256);
