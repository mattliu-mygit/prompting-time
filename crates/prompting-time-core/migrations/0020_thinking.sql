ALTER TABLE conversations ADD COLUMN thinking_preference_json TEXT NOT NULL
DEFAULT '{"kind":"auto"}' CHECK (length(CAST(thinking_preference_json AS BLOB)) <= 256);

-- Old runs keep the native default; Auto applies only to explicit new intent.
ALTER TABLE provider_runs ADD COLUMN thinking_decision_json TEXT NOT NULL
DEFAULT '{"preference":{"kind":"providerDefault"},"requestedEffort":null,"reason":"Uses the provider''s configured/default effort."}'
CHECK (length(CAST(thinking_decision_json AS BLOB)) <= 1024);
ALTER TABLE provider_runs ADD COLUMN thinking_configuration_json TEXT
CHECK (length(CAST(thinking_configuration_json AS BLOB)) <= 8192);
