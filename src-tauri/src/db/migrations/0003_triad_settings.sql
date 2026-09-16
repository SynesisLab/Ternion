-- Triad v1 (design §3): role assignments, policy knobs, sidecar toggles and
-- per-role keep-alives. Seeded so the Settings screens have real rows to edit;
-- absent rows still fall back to in-code defaults (router/config.rs).

INSERT INTO settings (key, value) VALUES
  ('triad.enabled', 'true'),
  ('triad.skip_router', 'false'),
  ('triad.role.herald', ''),
  ('triad.role.scout', ''),
  ('triad.role.titan', ''),
  ('triad.min_confidence', '0.65'),
  ('triad.deescalation_confidence', '0.8'),
  ('triad.sticky_turns', '1'),
  ('triad.scout_output_ceiling', '1024'),
  ('triad.handoff_recent_messages', '6'),
  ('triad.sidecar_titles', 'true'),
  ('triad.sidecar_suggestions', 'true'),
  ('triad.herald_timeout_ms', '4000'),
  ('herald.keep_alive', '24h'),
  ('triad.scout_keep_alive', '10m'),
  ('triad.titan_keep_alive', '3m')
  ON CONFLICT(key) DO NOTHING;