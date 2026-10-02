-- Absolute Number overrides were removed. Episode numbering is always derived
-- from the episode's real (LOCAL) number, and the active numbering mode decides
-- whether a filename uses the season-relative or the absolute number. The
-- per-episode override column is therefore unused — drop it.
ALTER TABLE episodes DROP COLUMN absolute_override;
