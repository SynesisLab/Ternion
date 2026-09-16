//! The policy engine (design §3.5): a deterministic wrapper around Herald's
//! opinion. Precedence: hard rules > Herald (confidence-gated) > heuristics >
//! scout default. Anti-flapping hysteresis (§3.6) lives in the de-escalation
//! gate: leaving Titan needs `sticky_turns` served, deescalation confidence,
//! and no titan-leaning flags.

use crate::{
    router::config::TriadConfig,
    types::{DecisionSource, RoutingDecision, RoutingFlags, Target},
};

/// Deterministic per-turn signals the orchestrator computes before Herald
/// runs. None of these need a model.
#[derive(Debug, Clone, Default)]
pub struct RouteContext {
    pub has_image: bool,
    pub message_chars: usize,
    pub message_words: usize,
    /// Tool results already in context (M2; always 0 until the tool runtime).
    pub prior_tool_results: usize,
    pub code_fence_max_lines: usize,
    pub code_keywords: bool,
    /// Consecutive trailing assistant turns produced by Titan (0 = on Scout).
    pub turns_on_titan: u32,
    /// Capability facts from the model registry; `None` = unknown → rules
    /// that need them are skipped rather than guessed.
    pub scout_vision: Option<bool>,
    pub scout_context_tokens: Option<u32>,
}

/// Keywords that signal architecture-level work (design H4).
const CODE_KEYWORDS: &[&str] = &["refactor", "architecture", "migrate", "migration"];

pub fn code_keywords_hit(message: &str) -> bool {
    let lower = message.to_lowercase();
    CODE_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

/// Size of the largest fenced code block (``` … ```) in the message.
pub fn code_fence_max_lines(message: &str) -> usize {
    let mut max = 0usize;
    let mut current: Option<usize> = None;
    for line in message.lines() {
        if line.trim_start().starts_with("```") {
            match current.take() {
                Some(len) => max = max.max(len),
                None => current = Some(0),
            }
        } else if current.is_some() {
            current = Some(current.unwrap_or(0) + 1);
        }
    }
    // Unclosed fence still counts what we saw.
    max.max(current.unwrap_or(0))
}

/// The full policy decision for one turn.
pub fn decide(
    cfg: &TriadConfig,
    ctx: &RouteContext,
    herald: Option<RoutingDecision>,
) -> RoutingDecision {
    // 1. Capability hard rules fire before anything else (§3.5 rule 2): they
    //    catch what a 0.6 B Herald structurally cannot (which models exist).
    let effective_image = ctx.has_image
        || herald.as_ref().map(|h| h.flags.vision).unwrap_or(false);
    if effective_image && ctx.scout_vision == Some(false) {
        let mut d = herald.unwrap_or_else(|| heuristic_decision(ctx));
        d.target = Target::Titan;
        d.flags.vision = true;
        d.reason = "image but scout lacks vision".into();
        d.source = DecisionSource::HardRule;
        return d;
    }
    if let Some(h) = &herald {
        if let Some(scout_ctx) = ctx.scout_context_tokens {
            if h.est_in_tokens > scout_ctx {
                let mut d = h.clone();
                d.target = Target::Titan;
                d.reason = "input exceeds scout context".into();
                d.source = DecisionSource::HardRule;
                return d;
            }
        }
    }

    // 2. Herald decision when it's confident enough…
    if let Some(h) = herald {
        if h.confidence >= cfg.min_confidence {
            let mut d = h;
            d.source = DecisionSource::Herald;
            if d.target == Target::Scout && ctx.turns_on_titan > 0 {
                // 3. De-escalation gate (§3.6 anti-flapping): leaving Titan
                //    needs the sticky window served, higher confidence, and
                //    no titan-leaning flags.
                if ctx.turns_on_titan < cfg.sticky_turns {
                    d.target = Target::Titan;
                    d.reason = "titan sticky window".into();
                } else if d.confidence < cfg.deescalation_confidence {
                    d.target = Target::Titan;
                    d.reason = "de-escalation confidence too low".into();
                } else if d.flags.multi_step || d.flags.long_form {
                    d.target = Target::Titan;
                    d.reason = "titan-leaning flags still set".into();
                }
            }
            return d;
        }
    }

    // 4. Heuristic router (Herald down / invalid JSON / low confidence), 5. default.
    heuristic_decision(ctx)
}

/// Pure fallback router, ordered rules H1–H6 (design §3.5 table).
fn heuristic_decision(ctx: &RouteContext) -> RoutingDecision {
    let long_form = ctx.message_chars > 2000 || ctx.message_words > 400;
    let code = ctx.code_fence_max_lines > 50 || ctx.code_keywords;

    let (target, reason) = if ctx.has_image {
        // H1 — the capability hard rule already forced titan when scout
        // can't see; here scout is capable or vision status is unknown.
        (Target::Scout, "image attachment")
    } else if long_form {
        (Target::Titan, "long message") // H2
    } else if ctx.prior_tool_results >= 2 {
        (Target::Titan, "multiple tool results") // H3
    } else if code {
        (Target::Titan, "large code task") // H4
    } else if ctx.turns_on_titan > 0 {
        (Target::Titan, "titan sticky window") // H5
    } else {
        (Target::Scout, "short/simple message") // H6
    };

    RoutingDecision {
        target,
        confidence: 0.5,
        complexity: (1 + usize::from(long_form) + usize::from(code)) as u8,
        reason: reason.into(),
        flags: RoutingFlags {
            vision: ctx.has_image,
            tools: false,
            code,
            long_form,
            multi_step: false,
            sensitive: false,
        },
        est_in_tokens: (ctx.message_chars / 4) as u32,
        est_out_tokens: 0,
        handoff_note: String::new(),
        source: DecisionSource::Heuristic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> RouteContext {
        RouteContext {
            scout_vision: Some(true),
            scout_context_tokens: Some(8192),
            ..Default::default()
        }
    }

    fn herald(target: Target, confidence: f32) -> RoutingDecision {
        RoutingDecision {
            target,
            confidence,
            complexity: 2,
            reason: "herald says".into(),
            flags: RoutingFlags::default(),
            est_in_tokens: 100,
            est_out_tokens: 0,
            handoff_note: "note".into(),
            source: DecisionSource::Herald,
        }
    }

    fn cfg() -> TriadConfig {
        TriadConfig::default()
    }

    #[test]
    fn h6_default_routes_scout() {
        let d = decide(&cfg(), &ctx(), None);
        assert_eq!(d.target, Target::Scout);
        assert_eq!(d.source, DecisionSource::Heuristic);
        assert_eq!(d.reason, "short/simple message");
    }

    #[test]
    fn h2_long_message_routes_titan() {
        let mut c = ctx();
        c.message_chars = 3000;
        assert_eq!(decide(&cfg(), &c, None).target, Target::Titan);
        let mut c = ctx();
        c.message_words = 401;
        assert_eq!(decide(&cfg(), &c, None).target, Target::Titan);
        // Right at the threshold stays scout.
        let mut c = ctx();
        c.message_chars = 2000;
        assert_eq!(decide(&cfg(), &c, None).target, Target::Scout);
    }

    #[test]
    fn h3_two_tool_results_route_titan() {
        let mut c = ctx();
        c.prior_tool_results = 2;
        assert_eq!(decide(&cfg(), &c, None).target, Target::Titan);
    }

    #[test]
    fn h4_code_fence_and_keywords_route_titan() {
        let mut c = ctx();
        c.code_fence_max_lines = 51;
        assert_eq!(decide(&cfg(), &c, None).target, Target::Titan);
        let mut c = ctx();
        c.code_keywords = true;
        assert_eq!(decide(&cfg(), &c, None).target, Target::Titan);
    }

    #[test]
    fn h5_sticky_routes_titan_when_no_herald() {
        let mut c = ctx();
        c.turns_on_titan = 1;
        assert_eq!(decide(&cfg(), &c, None).target, Target::Titan);
    }

    #[test]
    fn confident_herald_is_adopted() {
        let d = decide(&cfg(), &ctx(), Some(herald(Target::Titan, 0.9)));
        assert_eq!(d.target, Target::Titan);
        assert_eq!(d.source, DecisionSource::Herald);
        let d = decide(&cfg(), &ctx(), Some(herald(Target::Scout, 0.7)));
        assert_eq!(d.target, Target::Scout);
    }

    #[test]
    fn low_confidence_herald_falls_back_to_heuristics() {
        let mut c = ctx();
        c.message_chars = 3000; // heuristics must win, not the low-conf herald
        let d = decide(&cfg(), &c, Some(herald(Target::Scout, 0.3)));
        assert_eq!(d.target, Target::Titan);
        assert_eq!(d.source, DecisionSource::Heuristic);
    }

    #[test]
    fn vision_hard_rule_overrides_herald_scout() {
        let mut c = ctx();
        c.has_image = true;
        c.scout_vision = Some(false);
        let d = decide(&cfg(), &c, Some(herald(Target::Scout, 0.99)));
        assert_eq!(d.target, Target::Titan);
        assert_eq!(d.source, DecisionSource::HardRule);
        assert!(d.flags.vision);
        // Herald's vision flag alone (no actual attachment) triggers it too.
        let mut h = herald(Target::Scout, 0.99);
        h.flags.vision = true;
        let d = decide(&cfg(), &ctx(), Some(h.clone()));
        // scout_vision = Some(true) here, so no override — sanity check.
        assert_eq!(d.target, Target::Scout);
        let mut c = ctx();
        c.scout_vision = Some(false);
        let d = decide(&cfg(), &c, Some(h));
        assert_eq!(d.target, Target::Titan);
    }

    #[test]
    fn vision_with_capable_scout_routes_scout() {
        let mut c = ctx();
        c.has_image = true;
        let d = decide(&cfg(), &c, None);
        assert_eq!(d.target, Target::Scout);
        assert!(d.flags.vision);
    }

    #[test]
    fn context_hard_rule_overrides_herald_scout() {
        let mut h = herald(Target::Scout, 0.99);
        h.est_in_tokens = 9000;
        let d = decide(&cfg(), &ctx(), Some(h));
        assert_eq!(d.target, Target::Titan);
        assert_eq!(d.source, DecisionSource::HardRule);
    }

    #[test]
    fn unknown_capabilities_skip_hard_rules() {
        let mut c = ctx();
        c.has_image = true;
        c.scout_vision = None; // registry empty — can't judge
        let d = decide(&cfg(), &c, None);
        assert_eq!(d.target, Target::Scout);
    }

    #[test]
    fn deescalation_happy_path() {
        let mut c = ctx();
        c.turns_on_titan = 1; // sticky_turns = 1 → served
        let d = decide(&cfg(), &c, Some(herald(Target::Scout, 0.85)));
        assert_eq!(d.target, Target::Scout);
        assert_eq!(d.source, DecisionSource::Herald);
    }

    #[test]
    fn deescalation_blocked_by_sticky_window() {
        let mut cfg = cfg();
        cfg.sticky_turns = 3;
        let mut c = ctx();
        c.turns_on_titan = 2; // < sticky
        let d = decide(&cfg, &c, Some(herald(Target::Scout, 0.95)));
        assert_eq!(d.target, Target::Titan);
        assert_eq!(d.reason, "titan sticky window");
    }

    #[test]
    fn deescalation_blocked_by_low_confidence() {
        let mut c = ctx();
        c.turns_on_titan = 1;
        let d = decide(&cfg(), &c, Some(herald(Target::Scout, 0.75)));
        assert_eq!(d.target, Target::Titan);
        assert_eq!(d.reason, "de-escalation confidence too low");
    }

    #[test]
    fn deescalation_blocked_by_titan_flags() {
        let mut h = herald(Target::Scout, 0.9);
        h.flags.multi_step = true;
        let mut c = ctx();
        c.turns_on_titan = 1;
        let d = decide(&cfg(), &c, Some(h));
        assert_eq!(d.target, Target::Titan);
        assert_eq!(d.reason, "titan-leaning flags still set");
    }

    #[test]
    fn herald_titan_while_on_titan_stays() {
        let mut c = ctx();
        c.turns_on_titan = 5;
        let d = decide(&cfg(), &c, Some(herald(Target::Titan, 0.7)));
        assert_eq!(d.target, Target::Titan);
    }

    #[test]
    fn fence_counter_handles_boundaries() {
        assert_eq!(code_fence_max_lines("no code here"), 0);
        assert_eq!(code_fence_max_lines("```rust\nfn a() {}\nfn b() {}\n```"), 2);
        assert_eq!(code_fence_max_lines("text\n```\nl1\nl2\nl3"), 3, "unclosed fence counts");
        assert_eq!(code_fence_max_lines("```\na\n```\n```\nb\n```\n"), 1);
        assert_eq!(code_keywords_hit("refactor the auth module"), true);
        assert_eq!(code_keywords_hit("hello world"), false);
    }

    /// Golden sequence mirroring design §3.12 worked example ② → ④:
    /// escalate to titan, sticky continuation, confident de-escalation.
    #[test]
    fn worked_example_sequence() {
        let cfg = cfg();
        // Turn 1: refactor request — Herald titan.
        let mut c = ctx();
        c.code_keywords = true;
        let h = herald(Target::Titan, 0.91);
        assert_eq!(decide(&cfg, &c, Some(h)).target, Target::Titan);

        // Turn 2: follow-up, still titan (herald continuity), 1 turn on titan.
        let mut c = ctx();
        c.turns_on_titan = 1;
        let h = herald(Target::Titan, 0.8);
        assert_eq!(decide(&cfg, &c, Some(h)).target, Target::Titan);

        // Turn 3: trivial ask — Herald scout conf 0.83 ≥ deesc 0.80, sticky served.
        let mut c = ctx();
        c.turns_on_titan = 2;
        let h = herald(Target::Scout, 0.83);
        assert_eq!(decide(&cfg, &c, Some(h)).target, Target::Scout);
    }
}