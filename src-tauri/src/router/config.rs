//! Triad configuration (design §3.5/§3.10): role assignments and policy
//! knobs, loaded from the `settings` table. Every knob has an in-code default
//! so a missing row (fresh DB, pre-migration client) degrades gracefully.

use std::collections::HashMap;

use crate::{
    settings::{keys, SettingsCache},
    types::{RoutingFlags, Target},
};

/// Model ids assigned to each role. Empty string in settings = unassigned.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoleAssignments {
    pub herald: Option<String>,
    pub scout: Option<String>,
    pub titan: Option<String>,
}

impl RoleAssignments {
    /// The model for a routing target, if that role is assigned.
    pub fn model_for(&self, target: Target) -> Option<&str> {
        match target {
            Target::Scout => self.scout.as_deref(),
            Target::Titan => self.titan.as_deref(),
        }
    }

    /// Which role (if any) an explicit model id is assigned to — used for
    /// ribbon display and sticky-window counting on manually pinned chats.
    pub fn role_of(&self, model: &str) -> Option<Target> {
        if Some(model) == self.scout.as_deref() {
            Some(Target::Scout)
        } else if Some(model) == self.titan.as_deref() {
            Some(Target::Titan)
        } else {
            None
        }
    }
}

/// The Triad user pin on a conversation (`conversations.pinned_model`,
/// design §3.10): `auto` routes through Herald, `scout`/`titan` force a role,
/// any other value pins an explicit model (M0 behavior, "direct" mode).
#[derive(Debug, Clone, PartialEq)]
pub enum Pin {
    Auto,
    Role(Target),
    Model(String),
}

impl Pin {
    pub fn parse(pinned: Option<&str>) -> Self {
        match pinned {
            None | Some("") | Some("auto") => Pin::Auto,
            Some("scout") => Pin::Role(Target::Scout),
            Some("titan") => Pin::Role(Target::Titan),
            Some(model) => Pin::Model(model.to_string()),
        }
    }
}

/// Everything the router and orchestrator need from settings.
#[derive(Debug, Clone, PartialEq)]
pub struct TriadConfig {
    pub enabled: bool,
    /// Herald bypass (§3.10 "skip router"): pure manual mode.
    pub skip_router: bool,
    pub roles: RoleAssignments,
    pub min_confidence: f32,
    pub deescalation_confidence: f32,
    pub sticky_turns: u32,
    pub scout_output_ceiling: u32,
    pub handoff_recent_messages: u32,
    pub sidecar_titles: bool,
    pub sidecar_suggestions: bool,
    pub herald_timeout_ms: u64,
    pub herald_keep_alive: String,
    pub scout_keep_alive: String,
    pub titan_keep_alive: String,
    /// §3.11 adaptive tuning (experimental): pin overrides nudge the
    /// Herald escalation threshold per flag class. Off by default.
    pub adaptive_enabled: bool,
    /// flag class → delta applied on top of `min_confidence` for
    /// Herald-Titan decisions carrying that class.
    pub adaptive_bumps: HashMap<String, f32>,
}

impl Default for TriadConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            skip_router: false,
            roles: RoleAssignments::default(),
            min_confidence: 0.65,
            deescalation_confidence: 0.80,
            sticky_turns: 1,
            scout_output_ceiling: 1024,
            handoff_recent_messages: 6,
            sidecar_titles: true,
            sidecar_suggestions: true,
            // Generous: Herald is keep-alive-pinned so warm calls land well
            // under this; the timeout only bites on cold loads or a down model.
            herald_timeout_ms: 8000,
            herald_keep_alive: "24h".into(),
            scout_keep_alive: "10m".into(),
            titan_keep_alive: "3m".into(),
            adaptive_enabled: false,
            adaptive_bumps: HashMap::new(),
        }
    }
}

impl TriadConfig {
    /// Is the router actually usable? Needs at least one worker role assigned
    /// (or a fallback model supplied by the caller).
    pub fn routing_available(&self) -> bool {
        self.enabled && (self.roles.scout.is_some() || self.roles.titan.is_some())
    }

    /// §3.11: threshold delta for a Herald-Titan decision carrying these
    /// flags — the sum of bumps over the set classes, or the `plain` bump
    /// when none are. Zero without adaptive tuning.
    pub fn escalation_bump(&self, flags: &RoutingFlags) -> f32 {
        if !self.adaptive_enabled {
            return 0.0;
        }
        classes_of(flags)
            .iter()
            .filter_map(|c| self.adaptive_bumps.get(*c))
            .sum()
    }

    /// Load from the settings cache; missing keys take `Default` values.
    pub fn load(settings: &SettingsCache) -> Self {
        Self::from_pairs(&snapshot(settings))
    }

    /// Pure form for tests.
    pub fn from_pairs(pairs: &HashMap<String, String>) -> Self {
        let mut cfg = Self::default();
        let get = |key: &str| pairs.get(key).map(String::as_str);

        if let Some(v) = get(keys::TRIAD_ENABLED) {
            cfg.enabled = v != "false";
        }
        if let Some(v) = get(keys::TRIAD_SKIP_ROUTER) {
            cfg.skip_router = v == "true";
        }
        cfg.roles = RoleAssignments {
            herald: non_empty(get(keys::TRIAD_ROLE_HERALD)),
            scout: non_empty(get(keys::TRIAD_ROLE_SCOUT)),
            titan: non_empty(get(keys::TRIAD_ROLE_TITAN)),
        };
        if let Some(v) = get(keys::TRIAD_MIN_CONFIDENCE).and_then(parse_f32) {
            cfg.min_confidence = v;
        }
        if let Some(v) = get(keys::TRIAD_DEESCALATION_CONFIDENCE).and_then(parse_f32) {
            cfg.deescalation_confidence = v;
        }
        if let Some(v) = get(keys::TRIAD_STICKY_TURNS).and_then(parse_u32) {
            cfg.sticky_turns = v;
        }
        if let Some(v) = get(keys::TRIAD_SCOUT_OUTPUT_CEILING).and_then(parse_u32) {
            cfg.scout_output_ceiling = v;
        }
        if let Some(v) = get(keys::TRIAD_HANDOFF_RECENT_MESSAGES).and_then(parse_u32) {
            cfg.handoff_recent_messages = v;
        }
        if let Some(v) = get(keys::TRIAD_SIDECAR_TITLES) {
            cfg.sidecar_titles = v != "false";
        }
        if let Some(v) = get(keys::TRIAD_SIDECAR_SUGGESTIONS) {
            cfg.sidecar_suggestions = v != "false";
        }
        if let Some(v) = get(keys::TRIAD_HERALD_TIMEOUT_MS).and_then(parse_u32) {
            cfg.herald_timeout_ms = u64::from(v);
        }
        if let Some(v) = non_empty(get(keys::HERALD_KEEP_ALIVE)) {
            cfg.herald_keep_alive = v;
        }
        if let Some(v) = non_empty(get(keys::TRIAD_SCOUT_KEEP_ALIVE)) {
            cfg.scout_keep_alive = v;
        }
        if let Some(v) = non_empty(get(keys::TRIAD_TITAN_KEEP_ALIVE)) {
            cfg.titan_keep_alive = v;
        }
        if let Some(v) = get(keys::TRIAD_ADAPTIVE_ENABLED) {
            cfg.adaptive_enabled = v == "true";
        }
        // Bump rows are dynamic (one per flag class); anything under the
        // bump prefix that parses as f32 counts.
        for (k, v) in pairs.iter() {
            if let Some(flag) = k.strip_prefix(keys::ADAPTIVE_BUMP_PREFIX) {
                if let Some(delta) = parse_f32(v) {
                    cfg.adaptive_bumps.insert(flag.to_string(), delta);
                }
            }
        }
        cfg
    }
}

fn snapshot(settings: &SettingsCache) -> HashMap<String, String> {
    // Only the keys the router reads — cheap, and keeps tests honest about
    // which settings actually matter. The adaptive bump rows are dynamic
    // (one per flag class) and come along by prefix.
    const KEYS: &[&str] = &[
        keys::TRIAD_ENABLED,
        keys::TRIAD_SKIP_ROUTER,
        keys::TRIAD_ROLE_HERALD,
        keys::TRIAD_ROLE_SCOUT,
        keys::TRIAD_ROLE_TITAN,
        keys::TRIAD_MIN_CONFIDENCE,
        keys::TRIAD_DEESCALATION_CONFIDENCE,
        keys::TRIAD_STICKY_TURNS,
        keys::TRIAD_SCOUT_OUTPUT_CEILING,
        keys::TRIAD_HANDOFF_RECENT_MESSAGES,
        keys::TRIAD_SIDECAR_TITLES,
        keys::TRIAD_SIDECAR_SUGGESTIONS,
        keys::TRIAD_HERALD_TIMEOUT_MS,
        keys::HERALD_KEEP_ALIVE,
        keys::TRIAD_SCOUT_KEEP_ALIVE,
        keys::TRIAD_TITAN_KEEP_ALIVE,
        keys::TRIAD_ADAPTIVE_ENABLED,
    ];
    let mut out: HashMap<String, String> = KEYS
        .iter()
        .filter_map(|k| settings.get(k).map(|v| (k.to_string(), v)))
        .collect();
    for (k, v) in settings.prefix(keys::ADAPTIVE_BUMP_PREFIX) {
        out.insert(k, v);
    }
    out
}

/// The tunable classes a decision's flags belong to (§3.11): the set
/// titan-leaning flags, or `plain` when none are. Shared with the
/// pin-override learner in `commands/conversations.rs`.
pub fn classes_of(flags: &RoutingFlags) -> Vec<&'static str> {
    let mut classes = Vec::new();
    if flags.code {
        classes.push("code");
    }
    if flags.tools {
        classes.push("tools");
    }
    if flags.long_form {
        classes.push("long_form");
    }
    if flags.multi_step {
        classes.push("multi_step");
    }
    if classes.is_empty() {
        classes.push("plain");
    }
    classes
}

fn non_empty(v: Option<&str>) -> Option<String> {
    v.filter(|s| !s.trim().is_empty()).map(|s| s.trim().to_string())
}

fn parse_f32(s: &str) -> Option<f32> {
    s.trim().parse().ok()
}

fn parse_u32(s: &str) -> Option<u32> {
    s.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> HashMap<String, String> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn defaults_when_settings_missing() {
        let cfg = TriadConfig::from_pairs(&HashMap::new());
        assert!(cfg.enabled);
        assert!(!cfg.skip_router);
        assert_eq!(cfg.roles, RoleAssignments::default());
        assert!((cfg.min_confidence - 0.65).abs() < 1e-6);
        assert!((cfg.deescalation_confidence - 0.80).abs() < 1e-6);
        assert_eq!(cfg.sticky_turns, 1);
        assert_eq!(cfg.scout_output_ceiling, 1024);
        assert_eq!(cfg.handoff_recent_messages, 6);
        assert!(cfg.sidecar_titles && cfg.sidecar_suggestions);
        assert_eq!(cfg.herald_timeout_ms, 8000);
        assert_eq!(cfg.herald_keep_alive, "24h");
        assert_eq!(cfg.scout_keep_alive, "10m");
        assert_eq!(cfg.titan_keep_alive, "3m");
        assert!(!cfg.routing_available(), "no roles assigned yet");
    }

    #[test]
    fn parses_settings_and_ignores_blanks() {
        let cfg = TriadConfig::from_pairs(&pairs(&[
            ("triad.role.herald", "lfm2.5"),
            ("triad.role.scout", "  gemma3:4b  "),
            ("triad.role.titan", ""),
            ("triad.min_confidence", "0.5"),
            ("triad.sticky_turns", "3"),
            ("triad.enabled", "false"),
            ("triad.skip_router", "true"),
        ]));
        assert!(!cfg.enabled);
        assert!(cfg.skip_router);
        assert_eq!(cfg.roles.herald.as_deref(), Some("lfm2.5"));
        assert_eq!(cfg.roles.scout.as_deref(), Some("gemma3:4b"));
        assert_eq!(cfg.roles.titan, None);
        assert!((cfg.min_confidence - 0.5).abs() < 1e-6);
        assert_eq!(cfg.sticky_turns, 3);
    }

    #[test]
    fn routing_available_needs_enabled_plus_a_role() {
        let mut cfg = TriadConfig::default();
        cfg.roles.scout = Some("gemma3:4b".into());
        assert!(cfg.routing_available());
        cfg.enabled = false;
        assert!(!cfg.routing_available());
    }

    #[test]
    fn adaptive_bumps_load_and_apply_per_class() {
        let mut flags = RoutingFlags::default();
        assert!((TriadConfig::default().escalation_bump(&flags)).abs() < 1e-6);

        let mut cfg = TriadConfig::default();
        cfg.adaptive_enabled = true;
        // Still zero: nothing learned yet.
        assert!((cfg.escalation_bump(&flags)).abs() < 1e-6, "`plain` absent → 0");

        cfg.adaptive_bumps.insert("plain".into(), 0.05);
        assert!((cfg.escalation_bump(&flags) - 0.05).abs() < 1e-6);

        flags.code = true;
        flags.multi_step = true;
        assert!((cfg.escalation_bump(&flags)).abs() < 1e-6, "code flags don't see the plain bump");
        cfg.adaptive_bumps.insert("code".into(), 0.10);
        assert!((cfg.escalation_bump(&flags) - 0.10).abs() < 1e-6);

        // Disabled → always zero, bumps retained but inert.
        cfg.adaptive_enabled = false;
        assert!((cfg.escalation_bump(&flags)).abs() < 1e-6);
    }

    #[test]
    fn adaptive_settings_load_through_from_pairs() {
        let cfg = TriadConfig::from_pairs(&pairs(&[
            ("triad.adaptive.enabled", "true"),
            ("triad.adaptive.bump.code", "0.10"),
            ("triad.adaptive.bump.plain", "-0.05"),
            ("triad.adaptive.bump.garbage", "not-a-float"),
        ]));
        assert!(cfg.adaptive_enabled);
        assert!((cfg.adaptive_bumps["code"] - 0.10).abs() < 1e-6);
        assert!((cfg.adaptive_bumps["plain"] + 0.05).abs() < 1e-6);
        assert!(!cfg.adaptive_bumps.contains_key("garbage"));
    }

    #[test]
    fn pin_parse_covers_all_chip_values() {
        assert_eq!(Pin::parse(None), Pin::Auto);
        assert_eq!(Pin::parse(Some("")), Pin::Auto);
        assert_eq!(Pin::parse(Some("auto")), Pin::Auto);
        assert_eq!(Pin::parse(Some("scout")), Pin::Role(Target::Scout));
        assert_eq!(Pin::parse(Some("titan")), Pin::Role(Target::Titan));
        assert_eq!(
            Pin::parse(Some("gemma3:12b")),
            Pin::Model("gemma3:12b".into())
        );
    }

    #[test]
    fn role_of_matches_assignments() {
        let roles = RoleAssignments {
            herald: Some("lfm2.5".into()),
            scout: Some("gemma3:4b".into()),
            titan: Some("gemma3:12b".into()),
        };
        assert_eq!(roles.role_of("gemma3:4b"), Some(Target::Scout));
        assert_eq!(roles.role_of("gemma3:12b"), Some(Target::Titan));
        assert_eq!(roles.role_of("lfm2.5"), None, "herald is not a worker role");
        assert_eq!(roles.role_of("other"), None);
        assert_eq!(roles.model_for(Target::Scout), Some("gemma3:4b"));
        assert_eq!(roles.model_for(Target::Titan), Some("gemma3:12b"));
    }
}