-- Audit03 Phase 01: per-session settings blob.
-- Nullable; readers fall back to SessionSettings::default() when NULL.
-- Additive only — old rows continue to work with NULL.
alter table agent_sessions add column session_settings text;
