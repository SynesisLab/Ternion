/**
 * Traditional Chinese (繁體中文) catalog — the §9.5 first-class locale,
 * keyed `zh-TW` (the Windows/HK-CSL region tag users recognize).
 * Typed as a complete Record<I18nKey, string>: a key added to `en` without
 * a translation here is a compile error, so the catalog can never lag.
 *
 * Codenames (Ternion, Triad, Herald, Scout, Titan) stay in English — they
 * are product nouns, consistent with the model-picker labels.
 */

import type { I18nKey } from "./en";

export const zhTW: Record<I18nKey, string> = {
  "app.name": "Ternion",
  "app.tagline": "一個介面，三種模型。",

  "chat.input.placeholder": "與 Ternion 對話…",
  "chat.input.placeholder.offline": "無法連線 Ollama — 請檢查設定",
  "chat.input.send": "傳送",
  "chat.input.stop": "停止",

  "chat.thinking.active": "思考中…",
  "chat.thinking.done": "思考過程",
  "chat.status.connecting": "連線中…",
  "chat.status.loading_model": "載入模型中…",
  "chat.status.routing": "路由中…",

  "chat.meta.stopped": "已停止",
  "chat.meta.error": "錯誤",

  // Attachments (M3.6)
  "chat.attach.add": "附加圖片",
  "chat.attach.remove": "移除",
  "chat.attach.failed": "無法加入該圖片。",

  // Vision routing (M3.7)
  "chat.vision.gap": "此模型無法讀取圖片。",
  "chat.vision.fix": "切換至支援視覺的模型",

  // Tool runtime (M2)
  "chat.tools.activity": "工具活動",
  "chat.tools.running": "執行中…",
  "chat.tools.failed": "失敗",
  "workspace.title": "工作區",
  "workspace.none": "未綁定資料夾 — 模型無法存取檔案",
  "workspace.add": "綁定資料夾…",
  "workspace.addHint": "絕對資料夾路徑，例如 D:\\client-work",
  "workspace.bind": "綁定",
  "workspace.unbind": "解除綁定",
  "workspace.maxThree": "每個對話最多綁定 3 個工作區",
  "workspace.bound": "已綁定",

  // Permission matrix / approval modal (M2.6, §6.6)
  "approval.title": "需要授權",
  "approval.asking": "模型想要：",
  "approval.waiting": "等待授權…",
  "approval.diff": "建議變更",
  "approval.reject": "拒絕",
  "approval.edit": "就地編輯",
  "approval.allowOnce": "允許一次",
  "approval.allowSession": "本次工作階段允許",
  "approval.allowAlways": "此資料夾一律允許",
  "approval.deleteNote":
    "項目會移至資源回收筒 — 不會永久刪除。",
  "settings.permissions.title": "工具權限",
  "settings.permissions.always": "一律",
  "settings.permissions.reset": "重設為詢問",
  "settings.permissions.session": "工作階段授權",
  "settings.permissions.clearSession": "清除工作階段授權",
  "settings.permissions.empty": "沒有記住的授權。",
  "settings.shell.enabled": "啟用 Shell 工具",
  "settings.shell.enabledHint":
    "允許模型在已綁定的工作區執行 PowerShell 命令。每個命令都須經你核准。",

  // Triad routing (M1)
  "chat.pin.auto": "自動",
  "chat.pin.autoTitle": "Triad 路由器為每則訊息挑選模型",
  "chat.pin.scout": "Scout",
  "chat.pin.scoutTitle": "快速模型 — 快速問答與小型任務",
  "chat.pin.titan": "Titan",
  "chat.pin.titanTitle": "重量模型 — 程式碼、分析與長篇工作",
  "chat.pin.models": "模型",
  "chat.ribbon.by": "透過",
  "chat.ribbon.source.herald": "Herald",
  "chat.ribbon.source.heuristic": "啟發式",
  "chat.ribbon.source.hard_rule": "規則",
  "chat.ribbon.source.manual": "已釘選",
  "chat.ribbon.switched": "已切換",
  "chat.escalation.offer": "Scout 達到輸出上限 — 要以 Titan 接續嗎？",
  "chat.escalation.continue": "⚡ 以 Titan 接續",
  "chat.escalation.continuePrompt":
    "請從上次中斷處接續先前的回答。",
  "chat.suggestions.label": "接著試試",

  "routerlog.title": "路由紀錄",
  "routerlog.empty": "尚無路由決策 — 在自動模式下說點話吧。",
  "routerlog.open": "路由紀錄",
  "routerlog.override": "已釘選",
  "routerlog.note": "交接備註",
  "routerlog.estIn": "估計輸入",
  "routerlog.estOut": "估計輸出",

  "settings.tab.general": "一般",
  "settings.tab.endpoints": "端點",
  "settings.tab.models": "模型",
  "settings.tab.triad": "Triad",
  "settings.tab.permissions": "權限",

  // Endpoint profiles (M3, §5.1)
  "settings.endpoints.title": "端點設定檔",
  "settings.endpoints.builtin": "內建",
  "settings.endpoints.add": "新增端點",
  "settings.endpoints.kind": "類型",
  "settings.endpoints.kindOpenai": "OpenAI 相容",
  "settings.endpoints.kindOllama": "Ollama（原生）",
  "settings.endpoints.name": "名稱",
  "settings.endpoints.apiKey": "API 金鑰（儲存於 Windows 認證管理員）",
  "settings.endpoints.apiKeySet": "已儲存 API 金鑰",
  "settings.endpoints.apiKeyNone": "無 API 金鑰",
  "settings.endpoints.setKey": "儲存金鑰",
  "settings.endpoints.clearKey": "清除 API 金鑰",
  "settings.endpoints.test": "測試",
  "settings.endpoints.testing": "…",
  "settings.endpoints.models": "個模型",

  // Capability registry (M3, §5.4)
  "settings.models.title": "能力登錄 — 手動紀錄優先於探測結果",
  "settings.models.none": "尚未發現任何模型",
  "settings.models.contextTokens": "上限",
  "settings.models.save": "儲存",
  "settings.models.clear": "清除",
  "settings.models.clearHint": "移除手動紀錄，回退為探測結果",
  "settings.models.redetect": "重新探測",
  "settings.models.redetectHint": "透過 /api/show 重新探測能力與上下文長度",
  "settings.triad.enabled": "啟用 Triad 路由",
  "settings.triad.enabledHint": "Herald 為每則訊息分類並選擇 Scout 或 Titan。",
  "settings.triad.skipRouter": "略過路由器（一律使用所選模型）",
  "settings.triad.roles": "角色指派",
  "settings.triad.roleHerald": "Herald（分類器）",
  "settings.triad.roleScout": "Scout（快速工作者）",
  "settings.triad.roleTitan": "Titan（重量工作者）",
  "settings.triad.unassigned": "（未指派）",
  "settings.triad.policy": "政策",
  "settings.triad.minConfidence": "最低信心門檻",
  "settings.triad.deescalationConfidence": "降級信心門檻",
  "settings.triad.stickyTurns": "Titan 黏著回合數",
  "settings.triad.scoutOutputCeiling": "Scout 輸出上限（tokens）",
  "settings.triad.handoffRecentMessages": "交接近期訊息數",
  "settings.triad.heraldTimeoutMs": "Herald 逾時（ms）",
  "settings.triad.sidecars": "附屬任務（Herald）",
  "settings.triad.sidecarTitles": "精修對話標題",
  "settings.triad.sidecarSuggestions": "建議後續訊息",
  "settings.triad.report": "Triad 報表",
  "settings.triad.reportLoad": "顯示報表",
  "settings.triad.reportEmpty": "尚無路由回合。",
  "settings.triad.reportEscalation": "升級率",
  "settings.triad.reportOverride": "覆寫率",
  "settings.triad.reportHeraldLatency": "Herald 延遲（平均）",
  "settings.triad.reportPerRole": "各角色",
  "settings.triad.reportTimeSaved": "相較於一律用 Titan 的估計節省時間",
  "settings.triad.reportBaselineFallback": "尚無 Titan 回合 — 以 8 秒估計",
  "settings.triad.reportTokens": "tokens",
  "settings.triad.reportReset": "重設自適應",
  "settings.triad.adaptive": "自適應調校（實驗性）",
  "settings.triad.adaptiveHint":
    "與路由器相抵的釘選會微調該訊息類別的升級門檻。顯示於報表。",
  "settings.triad.reportAdaptiveOff": "自適應調校已關閉",

  "chat.empty.title": "一個介面。三種模型。",
  "chat.empty.subtitle": "每則訊息都有最合適的模型。在上方選擇模型，然後開始輸入。",
  "chat.empty.offline": "無法連上 Ollama — 點擊狀態燈號重試。",

  "status.ok": "已連線",
  "status.down": "離線",
  "status.unknown": "連線中…",

  "sidebar.new": "新對話",
  "sidebar.search": "搜尋對話…",
  "sidebar.untitled": "新對話",
  "sidebar.rename": "重新命名",
  "sidebar.delete": "刪除",
  "sidebar.deleteConfirm": "刪除此對話？此操作無法復原。",
  "sidebar.empty": "找不到對話",

  "header.renameTitle": "點擊以重新命名",

  "settings.title": "設定",
  "settings.baseUrl": "Ollama 基礎網址",
  "settings.test": "測試",
  "settings.testing": "測試中…",
  "settings.testOk": "端點可連線",
  "settings.testFail": "無法連線 — 請檢查網址並確認 Ollama 正在執行",
  "settings.temperature": "溫度",
  "settings.contextTokens": "上下文 tokens",
  "settings.keepAlive": "保持載入",
  "settings.closeToTray": "關閉時縮到系統匣（持續在背景執行）",
  "settings.privacy.localOnly": "僅本機模式",
  "settings.privacy.localOnlyHint":
    "全域停用雲端端點 — 路由僅使用本機模型。亦可於系統匣選單切換。",
  "settings.language": "語言",
  "settings.theme": "佈景主題",
  "settings.theme.dark": "深色",
  "settings.theme.light": "淺色",
  "settings.theme.system": "跟隨系統",
  "settings.reset": "重設預設值",
  "settings.saved": "已儲存",
  "settings.close": "取消",
  "settings.save": "儲存",

  // Quick capture window (§9.3)
  "quick.placeholder": "詢問 Ternion…",
  "quick.openMain": "在主視窗開啟",
  "quick.screenshot": "螢幕擷圖",
  "quick.offline": "沒有可連線的端點 — 請檢查設定",
  "quick.routeNote": "路由至",
} as const;