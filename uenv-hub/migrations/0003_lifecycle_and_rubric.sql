-- Environment lifecycle identity + rubric (gold-standard) contract.
--
-- Context: the Task Environment formerly named `math` is renamed to `qa`
-- (single-turn QA / classification verification). A rename cannot be a delete:
-- Workers pull `GET /envs/{env_type}/versions/latest` at boot and, when
-- `prewarm_on_startup` is set, treat a non-2xx as fatal. So the old name stays
-- resolvable and is instead *labelled* — hence `lifecycle` / `superseded_by`
-- rather than a row removal.
--
-- Rubric: a verification environment rewards by rule, so the rule is part of the
-- contract. `rubric_json` records the production scorer plus its measured
-- agreement against a reference implementation over a pinned corpus.
--
-- `latest_eligible` implements the publish gate. `latest` is resolved by taking
-- the highest non-yanked semver, so barring a version from `latest` requires a
-- per-version flag; the version itself stays fetchable by exact version, which
-- keeps the evidence auditable instead of hidden.

ALTER TABLE envs ADD COLUMN lifecycle TEXT NOT NULL DEFAULT 'active';
ALTER TABLE envs ADD COLUMN superseded_by TEXT;
ALTER TABLE envs ADD COLUMN compat_aliases TEXT;   -- JSON array of former names

CREATE INDEX idx_envs_lifecycle ON envs(lifecycle);

ALTER TABLE env_versions ADD COLUMN rubric_json TEXT;          -- JSON RubricSpec
ALTER TABLE env_versions ADD COLUMN latest_eligible INTEGER NOT NULL DEFAULT 1;
ALTER TABLE env_versions ADD COLUMN gate_notes TEXT;           -- JSON array
