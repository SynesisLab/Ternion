/**
 * User-facing strings. M0 hard-codes English behind a typed accessor so the
 * M4 i18next swap is mechanical (add more dicts + a runtime locale).
 */

export const en = {
  "app.name": "Ternion",
  "app.tagline": "One interface. Three models.",

  "chat.input.placeholder": "Message Ternion…",
  "chat.input.placeholder.offline": "Ollama is unreachable — check settings",
  "chat.input.send": "Send",
  "chat.input.stop": "Stop",

  "chat.thinking.active": "Thinking…",
  "chat.thinking.done": "Thought process",
  "chat.status.connecting": "Connecting…",
  "chat.status.loading_model": "Loading model…",
  "chat.status.routing": "Routing…",

  "chat.meta.stopped": "Stopped",
  "chat.meta.error": "Error",

  // Tool runtime (M2)
  "chat.tools.activity": "Tool activity",
  "chat.tools.running": "running…",
  "chat.tools.failed": "failed",
  "workspace.title": "Workspace",
  "workspace.none": "No folder bound — the model has no file access",
  "workspace.add": "Bind folder…",
  "workspace.addHint": "Absolute folder path, e.g. D:\\client-work",
  "workspace.bind": "Bind",
  "workspace.unbind": "Unbind",
  "workspace.maxThree": "A chat can bind at most 3 workspaces",
  "workspace.bound": "bound",

  // Triad routing (M1)
  "chat.pin.auto": "Auto",
  "chat.pin.autoTitle": "Triad router picks the model per message",
  "chat.pin.scout": "Scout",
  "chat.pin.scoutTitle": "Fast model — quick questions and small tasks",
  "chat.pin.titan": "Titan",
  "chat.pin.titanTitle": "Heavy model — code, analysis, long work",
  "chat.pin.models": "Models",
  "chat.ribbon.by": "via",
  "chat.ribbon.source.herald": "Herald",
  "chat.ribbon.source.heuristic": "Heuristic",
  "chat.ribbon.source.hard_rule": "Rule",
  "chat.ribbon.source.manual": "Pinned",
  "chat.ribbon.switched": "switched",
  "chat.escalation.offer": "Scout hit its output limit — continue with Titan?",
  "chat.escalation.continue": "⚡ Continue with Titan",
  "chat.escalation.continuePrompt":
    "Please continue your previous answer from where it stopped.",
  "chat.suggestions.label": "Try next",

  "routerlog.title": "Router log",
  "routerlog.empty": "No routing decisions yet — say something with Auto on.",
  "routerlog.open": "Router log",
  "routerlog.override": "pinned",
  "routerlog.note": "Handoff note",
  "routerlog.estIn": "est. in",
  "routerlog.estOut": "est. out",

  "settings.tab.general": "General",
  "settings.tab.triad": "Triad",
  "settings.triad.enabled": "Enable Triad routing",
  "settings.triad.enabledHint":
    "Herald classifies each message and picks Scout or Titan.",
  "settings.triad.skipRouter": "Skip router (always use the picked model)",
  "settings.triad.roles": "Role assignments",
  "settings.triad.roleHerald": "Herald (classifier)",
  "settings.triad.roleScout": "Scout (fast worker)",
  "settings.triad.roleTitan": "Titan (heavy worker)",
  "settings.triad.unassigned": "(unassigned)",
  "settings.triad.policy": "Policy",
  "settings.triad.minConfidence": "Min. confidence",
  "settings.triad.deescalationConfidence": "De-escalation confidence",
  "settings.triad.stickyTurns": "Sticky turns on Titan",
  "settings.triad.scoutOutputCeiling": "Scout output ceiling (tokens)",
  "settings.triad.handoffRecentMessages": "Handoff recent messages",
  "settings.triad.heraldTimeoutMs": "Herald timeout (ms)",
  "settings.triad.sidecars": "Sidecar tasks (Herald)",
  "settings.triad.sidecarTitles": "Refine chat titles",
  "settings.triad.sidecarSuggestions": "Suggest follow-up messages",

  "chat.empty.title": "One interface. Three models.",
  "chat.empty.subtitle":
    "The right model for every message. Pick a model above, then start typing.",
  "chat.empty.offline": "Can't reach Ollama — click the status dot to retry.",

  "status.ok": "Connected",
  "status.down": "Offline",
  "status.unknown": "Connecting…",

  "sidebar.new": "New chat",
  "sidebar.search": "Search chats…",
  "sidebar.untitled": "New chat",
  "sidebar.rename": "Rename",
  "sidebar.delete": "Delete",
  "sidebar.deleteConfirm": "Delete this conversation? This can't be undone.",
  "sidebar.empty": "No chats found",

  "header.renameTitle": "Click to rename",

  "settings.title": "Settings",
  "settings.baseUrl": "Ollama base URL",
  "settings.test": "Test",
  "settings.testing": "Testing…",
  "settings.testOk": "Endpoint reachable",
  "settings.testFail": "Unreachable — check the URL and that Ollama is running",
  "settings.temperature": "Temperature",
  "settings.contextTokens": "Context tokens",
  "settings.keepAlive": "Keep alive",
  "settings.closeToTray": "Close to tray (keep running in background)",
  "settings.reset": "Reset defaults",
  "settings.saved": "Saved",
  "settings.close": "Cancel",
  "settings.save": "Save",
} as const;

export type I18nKey = keyof typeof en;