ALTER TABLE approvals
ADD COLUMN details_json TEXT
CHECK (details_json IS NULL OR json_valid(details_json));
