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