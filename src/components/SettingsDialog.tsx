import { useEffect, useState } from "react";

import {
  clearEndpointApiKey,
  clearModelRecord,
  clearSessionPermissions,
  deleteEndpointProfile,
  getSetting,
  listEndpointProfiles,
  listModels,
  listToolPermissions,
  saveEndpointProfile,
  saveModelRecord,
  setEndpointApiKey,
  setSetting,
  setToolPermission,
  testEndpoint,
  triadReport,
  verifyModel,
  type EndpointProfile,
  type EndpointTestResult,
  type ToolPermissionRow,
} from "../lib/ipc";
import type { ModelInfo, TriadReport } from "../types/chat";
import { settingsKeys } from "../lib/settingsKeys";
import { t } from "../i18n";
import { useChatStore } from "../store/chatStore";

const DEFAULT_BASE_URL = "http://127.0.0.1:11434";
const DEFAULT_TEMPERATURE = "0.7";
const DEFAULT_CONTEXT_TOKENS = "8192";
const DEFAULT_KEEP_ALIVE = "10m";

// Triad defaults — mirror TriadConfig::default() in router/config.rs.
const DEFAULT_MIN_CONFIDENCE = "0.65";
const DEFAULT_DEESCALATION_CONFIDENCE = "0.80";
const DEFAULT_STICKY_TURNS = "1";
const DEFAULT_SCOUT_CEILING = "1024";
const DEFAULT_HANDOFF_RECENT = "6";
const DEFAULT_HERALD_TIMEOUT = "8000";

type TestState = "idle" | "testing" | "ok" | "fail";
type Tab = "general" | "endpoints" | "models" | "triad" | "permissions";

export function SettingsDialog({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const refreshModels = useChatStore((s) => s.refreshModels);
  const models = useChatStore((s) => s.models);

  // -- general --------------------------------------------------------------
  const [tab, setTab] = useState<Tab>("general");
  const [baseUrl, setBaseUrl] = useState(DEFAULT_BASE_URL);
  const [temperature, setTemperature] = useState(DEFAULT_TEMPERATURE);
  const [contextTokens, setContextTokens] = useState(DEFAULT_CONTEXT_TOKENS);
  const [keepAlive, setKeepAlive] = useState(DEFAULT_KEEP_ALIVE);
  const [closeToTray, setCloseToTray] = useState(true);
  const [shellEnabled, setShellEnabled] = useState(false);
  const [test, setTest] = useState<TestState>("idle");
  const [saved, setSaved] = useState(false);

  // -- triad ----------------------------------------------------------------
  const [triadEnabled, setTriadEnabled] = useState(true);
  const [skipRouter, setSkipRouter] = useState(false);
  const [roleHerald, setRoleHerald] = useState("");
  const [roleScout, setRoleScout] = useState("");
  const [roleTitan, setRoleTitan] = useState("");
  const [minConfidence, setMinConfidence] = useState(DEFAULT_MIN_CONFIDENCE);
  const [deescalationConfidence, setDeescalationConfidence] = useState(
    DEFAULT_DEESCALATION_CONFIDENCE,
  );
  const [stickyTurns, setStickyTurns] = useState(DEFAULT_STICKY_TURNS);
  const [scoutCeiling, setScoutCeiling] = useState(DEFAULT_SCOUT_CEILING);
  const [handoffRecent, setHandoffRecent] = useState(DEFAULT_HANDOFF_RECENT);
  const [heraldTimeout, setHeraldTimeout] = useState(DEFAULT_HERALD_TIMEOUT);
  const [sidecarTitles, setSidecarTitles] = useState(true);
  const [sidecarSuggestions, setSidecarSuggestions] = useState(true);

  // -- permissions (§6.6) ---------------------------------------------------
  const [grants, setGrants] = useState<ToolPermissionRow[]>([]);

  // -- endpoints (§5.1) -----------------------------------------------------
  const [profiles, setProfiles] = useState<EndpointProfile[]>([]);

  useEffect(() => {
    if (!open) return;
    void listToolPermissions()
      .then(setGrants)
      .catch(() => setGrants([]));
    void listEndpointProfiles()
      .then(setProfiles)
      .catch(() => setProfiles([]));
    void (async () => {
      const [url, temp, ctx, ka, tray, shell] = await Promise.all([
        getSetting(settingsKeys.ollamaBaseUrl),
        getSetting(settingsKeys.chatTemperature),
        getSetting(settingsKeys.chatContextTokens),
        getSetting(settingsKeys.chatKeepAlive),
        getSetting(settingsKeys.appCloseToTray),
        getSetting(settingsKeys.toolsShellEnabled),
      ]);
      setBaseUrl(url ?? DEFAULT_BASE_URL);
      setTemperature(temp ?? DEFAULT_TEMPERATURE);
      setContextTokens(ctx ?? DEFAULT_CONTEXT_TOKENS);
      setKeepAlive(ka ?? DEFAULT_KEEP_ALIVE);
      setCloseToTray((tray ?? "true") !== "false");
      setShellEnabled(shell === "true");

      const k = settingsKeys;
      const triad = await Promise.all([
        getSetting(k.triadEnabled),
        getSetting(k.triadSkipRouter),
        getSetting(k.triadRoleHerald),
        getSetting(k.triadRoleScout),
        getSetting(k.triadRoleTitan),
        getSetting(k.triadMinConfidence),
        getSetting(k.triadDeescalationConfidence),
        getSetting(k.triadStickyTurns),
        getSetting(k.triadScoutOutputCeiling),
        getSetting(k.triadHandoffRecentMessages),
        getSetting(k.triadHeraldTimeoutMs),
        getSetting(k.triadSidecarTitles),
        getSetting(k.triadSidecarSuggestions),
      ]);
      setTriadEnabled((triad[0] ?? "true") !== "false");
      setSkipRouter(triad[1] === "true");
      setRoleHerald(triad[2] ?? "");
      setRoleScout(triad[3] ?? "");
      setRoleTitan(triad[4] ?? "");
      setMinConfidence(triad[5] ?? DEFAULT_MIN_CONFIDENCE);
      setDeescalationConfidence(triad[6] ?? DEFAULT_DEESCALATION_CONFIDENCE);
      setStickyTurns(triad[7] ?? DEFAULT_STICKY_TURNS);
      setScoutCeiling(triad[8] ?? DEFAULT_SCOUT_CEILING);
      setHandoffRecent(triad[9] ?? DEFAULT_HANDOFF_RECENT);
      setHeraldTimeout(triad[10] ?? DEFAULT_HERALD_TIMEOUT);
      setSidecarTitles((triad[11] ?? "true") !== "false");
      setSidecarSuggestions((triad[12] ?? "true") !== "false");
    })();
  }, [open]);

  if (!open) return null;

  const resetDefaults = () => {
    setBaseUrl(DEFAULT_BASE_URL);
    setTemperature(DEFAULT_TEMPERATURE);
    setContextTokens(DEFAULT_CONTEXT_TOKENS);
    setKeepAlive(DEFAULT_KEEP_ALIVE);
    setCloseToTray(true);
    setShellEnabled(false);
    setTriadEnabled(true);
    setSkipRouter(false);
    setRoleHerald("");
    setRoleScout("");
    setRoleTitan("");
    setMinConfidence(DEFAULT_MIN_CONFIDENCE);
    setDeescalationConfidence(DEFAULT_DEESCALATION_CONFIDENCE);
    setStickyTurns(DEFAULT_STICKY_TURNS);
    setScoutCeiling(DEFAULT_SCOUT_CEILING);
    setHandoffRecent(DEFAULT_HANDOFF_RECENT);
    setHeraldTimeout(DEFAULT_HERALD_TIMEOUT);
    setSidecarTitles(true);
    setSidecarSuggestions(true);
  };

  const runTest = async () => {
    setTest("testing");
    try {
      const found = await listModels();
      setTest(found.length > 0 ? "ok" : "fail");
    } catch {
      setTest("fail");
    }
  };

  const save = async () => {
    const url = baseUrl.trim().replace(/\/+$/, "");
    const k = settingsKeys;
    const writes: Array<Promise<unknown>> = [
      setSetting(k.ollamaBaseUrl, url),
      setSetting(k.chatTemperature, temperature.trim() || DEFAULT_TEMPERATURE),
      setSetting(
        k.chatContextTokens,
        contextTokens.trim() || DEFAULT_CONTEXT_TOKENS,
      ),
      setSetting(k.chatKeepAlive, keepAlive.trim() || DEFAULT_KEEP_ALIVE),
      setSetting(k.appCloseToTray, String(closeToTray)),
      setSetting(k.toolsShellEnabled, String(shellEnabled)),
      setSetting(k.triadEnabled, String(triadEnabled)),
      setSetting(k.triadSkipRouter, String(skipRouter)),
      setSetting(k.triadRoleHerald, roleHerald.trim()),
      setSetting(k.triadRoleScout, roleScout.trim()),
      setSetting(k.triadRoleTitan, roleTitan.trim()),
      setSetting(k.triadMinConfidence, minConfidence.trim() || DEFAULT_MIN_CONFIDENCE),
      setSetting(
        k.triadDeescalationConfidence,
        deescalationConfidence.trim() || DEFAULT_DEESCALATION_CONFIDENCE,
      ),
      setSetting(k.triadStickyTurns, stickyTurns.trim() || DEFAULT_STICKY_TURNS),
      setSetting(k.triadScoutOutputCeiling, scoutCeiling.trim() || DEFAULT_SCOUT_CEILING),
      setSetting(
        k.triadHandoffRecentMessages,
        handoffRecent.trim() || DEFAULT_HANDOFF_RECENT,
      ),
      setSetting(k.triadHeraldTimeoutMs, heraldTimeout.trim() || DEFAULT_HERALD_TIMEOUT),
      setSetting(k.triadSidecarTitles, String(sidecarTitles)),
      setSetting(k.triadSidecarSuggestions, String(sidecarSuggestions)),
    ];
    await Promise.all(writes).catch(() => {});
    await refreshModels();
    setSaved(true);
    window.setTimeout(() => setSaved(false), 1500);
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="w-full max-w-lg rounded-xl border border-[color:var(--color-edge)] bg-[color:var(--color-panel)] p-5 shadow-2xl">
        <div className="mb-4 flex items-center justify-between">
          <div className="flex items-center gap-1 rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-bg)] p-0.5">
            <TabButton active={tab === "general"} onClick={() => setTab("general")}>
              {t("settings.tab.general")}
            </TabButton>
            <TabButton active={tab === "endpoints"} onClick={() => setTab("endpoints")}>
              {t("settings.tab.endpoints")}
            </TabButton>
            <TabButton active={tab === "models"} onClick={() => setTab("models")}>
              {t("settings.tab.models")}
            </TabButton>
            <TabButton active={tab === "triad"} onClick={() => setTab("triad")}>
              {t("settings.tab.triad")}
            </TabButton>
            <TabButton
              active={tab === "permissions"}
              onClick={() => setTab("permissions")}
            >
              {t("settings.tab.permissions")}
            </TabButton>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded px-2 text-sm text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)]"
          >
            ✕
          </button>
        </div>

        {tab === "general" ? (
          <div className="space-y-4">
            {/* Base URL */}
            <div>
              <label className="mb-1 block text-xs font-medium text-[color:var(--color-muted)]">
                {t("settings.baseUrl")}
              </label>
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  value={baseUrl}
                  onChange={(e) => setBaseUrl(e.target.value)}
                  className="min-w-0 flex-1 rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-bg)] px-2.5 py-1.5 text-sm outline-none focus:border-[color:var(--color-accent)]"
                />
                <button
                  type="button"
                  onClick={() => void runTest()}
                  className="shrink-0 rounded-lg border border-[color:var(--color-edge)] px-3 py-1.5 text-sm text-[color:var(--color-ink)] hover:border-[color:var(--color-accent)]"
                >
                  {t("settings.test")}
                </button>
              </div>
              <div className="mt-1 h-4 text-xs">
                {test === "testing" && (
                  <span className="text-[color:var(--color-muted)]">
                    {t("settings.testing")}
                  </span>
                )}
                {test === "ok" && (
                  <span className="text-[color:var(--color-accent-2)]">
                    ✓ {t("settings.testOk")}
                  </span>
                )}
                {test === "fail" && (
                  <span className="text-[color:var(--color-danger)]">
                    ✕ {t("settings.testFail")}
                  </span>
                )}
              </div>
            </div>

            {/* Generation defaults */}
            <div className="grid grid-cols-3 gap-2">
              <Field
                label={t("settings.temperature")}
                value={temperature}
                onChange={setTemperature}
              />
              <Field
                label={t("settings.contextTokens")}
                value={contextTokens}
                onChange={setContextTokens}
              />
              <Field
                label={t("settings.keepAlive")}
                value={keepAlive}
                onChange={setKeepAlive}
              />
            </div>

            {/* Behavior */}
            <label className="flex items-center gap-2 text-sm text-[color:var(--color-ink)]">
              <input
                type="checkbox"
                checked={closeToTray}
                onChange={(e) => setCloseToTray(e.target.checked)}
                className="accent-[color:var(--color-accent)]"
              />
              {t("settings.closeToTray")}
            </label>

            {/* Shell tool opt-in (§6.2) */}
            <label className="block">
              <span className="flex items-center gap-2 text-sm text-[color:var(--color-ink)]">
                <input
                  type="checkbox"
                  checked={shellEnabled}
                  onChange={(e) => setShellEnabled(e.target.checked)}
                  className="accent-[color:var(--color-accent)]"
                />
                {t("settings.shell.enabled")}
              </span>
              <span className="mt-1 block pl-6 text-xs text-[color:var(--color-muted)]">
                {t("settings.shell.enabledHint")}
              </span>
            </label>
          </div>
        ) : tab === "endpoints" ? (
          <EndpointsTab
            profiles={profiles}
            onReload={async () => {
              setProfiles(await listEndpointProfiles().catch(() => []));
            }}
          />
        ) : tab === "models" ? (
          <ModelsTab profiles={profiles} />
        ) : tab === "triad" ? (
          <div className="space-y-4">
            <TriadReportSection />

            <label className="block">
              <span className="flex items-center gap-2 text-sm text-[color:var(--color-ink)]">
                <input
                  type="checkbox"
                  checked={triadEnabled}
                  onChange={(e) => setTriadEnabled(e.target.checked)}
                  className="accent-[color:var(--color-accent)]"
                />
                {t("settings.triad.enabled")}
              </span>
              <span className="mt-1 block pl-6 text-xs text-[color:var(--color-muted)]">
                {t("settings.triad.enabledHint")}
              </span>
            </label>

            <label className="flex items-center gap-2 text-sm text-[color:var(--color-ink)]">
              <input
                type="checkbox"
                checked={skipRouter}
                onChange={(e) => setSkipRouter(e.target.checked)}
                className="accent-[color:var(--color-accent)]"
              />
              {t("settings.triad.skipRouter")}
            </label>

            {/* Role assignments */}
            <div>
              <div className="mb-1.5 text-xs font-medium text-[color:var(--color-muted)]">
                {t("settings.triad.roles")}
              </div>
              <div className="grid grid-cols-3 gap-2">
                <RoleSelect
                  label={t("settings.triad.roleHerald")}
                  models={models}
                  value={roleHerald}
                  onChange={setRoleHerald}
                />
                <RoleSelect
                  label={t("settings.triad.roleScout")}
                  models={models}
                  value={roleScout}
                  onChange={setRoleScout}
                />
                <RoleSelect
                  label={t("settings.triad.roleTitan")}
                  models={models}
                  value={roleTitan}
                  onChange={setRoleTitan}
                />
              </div>
            </div>

            {/* Policy knobs */}
            <div>
              <div className="mb-1.5 text-xs font-medium text-[color:var(--color-muted)]">
                {t("settings.triad.policy")}
              </div>
              <div className="grid grid-cols-3 gap-2">
                <Field
                  label={t("settings.triad.minConfidence")}
                  value={minConfidence}
                  onChange={setMinConfidence}
                />
                <Field
                  label={t("settings.triad.deescalationConfidence")}
                  value={deescalationConfidence}
                  onChange={setDeescalationConfidence}
                />
                <Field
                  label={t("settings.triad.stickyTurns")}
                  value={stickyTurns}
                  onChange={setStickyTurns}
                />
                <Field
                  label={t("settings.triad.scoutOutputCeiling")}
                  value={scoutCeiling}
                  onChange={setScoutCeiling}
                />
                <Field
                  label={t("settings.triad.handoffRecentMessages")}
                  value={handoffRecent}
                  onChange={setHandoffRecent}
                />
                <Field
                  label={t("settings.triad.heraldTimeoutMs")}
                  value={heraldTimeout}
                  onChange={setHeraldTimeout}
                />
              </div>
            </div>

            {/* Sidecars */}
            <div>
              <div className="mb-1.5 text-xs font-medium text-[color:var(--color-muted)]">
                {t("settings.triad.sidecars")}
              </div>
              <label className="flex items-center gap-2 text-sm text-[color:var(--color-ink)]">
                <input
                  type="checkbox"
                  checked={sidecarTitles}
                  onChange={(e) => setSidecarTitles(e.target.checked)}
                  className="accent-[color:var(--color-accent)]"
                />
                {t("settings.triad.sidecarTitles")}
              </label>
              <label className="mt-1 flex items-center gap-2 text-sm text-[color:var(--color-ink)]">
                <input
                  type="checkbox"
                  checked={sidecarSuggestions}
                  onChange={(e) => setSidecarSuggestions(e.target.checked)}
                  className="accent-[color:var(--color-accent)]"
                />
                {t("settings.triad.sidecarSuggestions")}
              </label>
            </div>
          </div>
        ) : (
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <div className="text-xs font-medium text-[color:var(--color-muted)]">
                {t("settings.permissions.title")}
              </div>
              <button
                type="button"
                onClick={() => {
                  void clearSessionPermissions();
                }}
                className="rounded-lg border border-[color:var(--color-edge)] px-2.5 py-1 text-xs text-[color:var(--color-ink)] hover:border-[color:var(--color-accent)]"
              >
                {t("settings.permissions.clearSession")}
              </button>
            </div>
            {grants.length === 0 ? (
              <div className="text-sm text-[color:var(--color-muted)]">
                {t("settings.permissions.empty")}
              </div>
            ) : (
              <div className="divide-y divide-[color:var(--color-edge)] rounded-lg border border-[color:var(--color-edge)]">
                {grants.map((g) => (
                  <div
                    key={`${g.tool}:${g.root}`}
                    className="flex items-center justify-between gap-3 px-3 py-2"
                  >
                    <div className="min-w-0">
                      <div className="truncate text-sm text-[color:var(--color-ink)]">
                        {g.tool}
                      </div>
                      <div className="truncate font-mono text-xs text-[color:var(--color-muted)]">
                        {g.root}
                      </div>
                    </div>
                    <div className="flex shrink-0 items-center gap-2">
                      <span className="rounded bg-[color:var(--color-bg)] px-1.5 py-0.5 text-xs text-[color:var(--color-accent-2)]">
                        {t("settings.permissions.always")}
                      </span>
                      <button
                        type="button"
                        onClick={() => {
                          void setToolPermission(g.tool, g.root, "ask").then(() =>
                            listToolPermissions()
                              .then(setGrants)
                              .catch(() => {}),
                          );
                        }}
                        className="rounded-lg border border-[color:var(--color-edge)] px-2.5 py-1 text-xs text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)]"
                      >
                        {t("settings.permissions.reset")}
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}

        <div className="mt-5 flex items-center justify-between">
          <button
            type="button"
            onClick={resetDefaults}
            className="text-xs text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)]"
          >
            {t("settings.reset")}
          </button>
          <div className="flex items-center gap-2">
            {saved && (
              <span className="text-xs text-[color:var(--color-accent-2)]">
                {t("settings.saved")}
              </span>
            )}
            <button
              type="button"
              onClick={onClose}
              className="rounded-lg border border-[color:var(--color-edge)] px-3 py-1.5 text-sm text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)]"
            >
              {t("settings.close")}
            </button>
            <button
              type="button"
              onClick={() => void save()}
              className="rounded-lg bg-[color:var(--color-accent)] px-4 py-1.5 text-sm font-medium text-white hover:opacity-90"
            >
              {t("settings.save")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`rounded-md px-3 py-1 text-xs font-medium transition-colors ${
        active
          ? "bg-[color:var(--color-accent)] text-white"
          : "text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)]"
      }`}
    >
      {children}
    </button>
  );
}

/** §3.11 Triad report — fetched on demand; rates derived from counts. */
function TriadReportSection() {
  const [report, setReport] = useState<TriadReport | null>(null);
  const [loading, setLoading] = useState(false);

  const load = async () => {
    setLoading(true);
    await triadReport()
      .then(setReport)
      .catch(() => setReport(null))
      .finally(() => setLoading(false));
  };

  return (
    <div className="rounded-lg border border-[color:var(--color-edge)] p-3">
      <div className="flex items-center justify-between">
        <div className="text-xs font-medium text-[color:var(--color-muted)]">
          {t("settings.triad.report")}
        </div>
        <button
          type="button"
          onClick={() => void load()}
          className="rounded-lg border border-[color:var(--color-edge)] px-2.5 py-1 text-xs text-[color:var(--color-ink)] hover:border-[color:var(--color-accent)]"
        >
          {loading ? "…" : t("settings.triad.reportLoad")}
        </button>
      </div>

      {report && report.totalTurns === 0 && (
        <div className="mt-2 text-xs text-[color:var(--color-muted)]">
          {t("settings.triad.reportEmpty")}
        </div>
      )}

      {report && report.totalTurns > 0 && <ReportBody report={report} />}
    </div>
  );
}

function ReportBody({ report }: { report: TriadReport }) {
  const auto = report.heraldTurns + report.heuristicTurns + report.hardRuleTurns;
  const escRate = auto > 0 ? Math.round((report.escalations / auto) * 100) : null;
  const ovrRate =
    report.totalTurns > 0
      ? Math.round((report.overrides / report.totalTurns) * 100)
      : null;

  return (
    <div className="mt-2.5 space-y-1.5 text-xs text-[color:var(--color-ink)]">
      <div className="flex flex-wrap gap-x-4 gap-y-1">
        <span>
          {report.totalTurns} turns · {report.heraldTurns} herald ·{" "}
          {report.heuristicTurns} heur · {report.hardRuleTurns} rule ·{" "}
          {report.manualTurns} manual
        </span>
      </div>
      <div className="flex flex-wrap gap-x-4 gap-y-1 text-[color:var(--color-muted)]">
        <span>
          {t("settings.triad.reportEscalation")}: {report.escalations}/{auto}
          {escRate != null && ` (${escRate}%)`}
        </span>
        <span>De-esc: {report.deescalations}</span>
        <span>
          {t("settings.triad.reportOverride")}: {report.overrides}/{report.totalTurns}
          {ovrRate != null && ` (${ovrRate}%)`}
        </span>
        {report.avgHeraldLatencyMs != null && (
          <span>
            {t("settings.triad.reportHeraldLatency")}:{" "}
            {fmtMs(report.avgHeraldLatencyMs)}
          </span>
        )}
      </div>

      {report.roles.length > 0 && (
        <table className="w-full border-collapse">
          <thead>
            <tr className="text-left text-[10px] uppercase tracking-wide text-[color:var(--color-muted)]">
              <th className="py-1 pr-2 font-medium">role</th>
              <th className="py-1 pr-2 font-medium">turns</th>
              <th className="py-1 pr-2 font-medium">latency</th>
              <th className="py-1 font-medium">{t("settings.triad.reportTokens")}</th>
            </tr>
          </thead>
          <tbody>
            {report.roles.map((r) => (
              <tr key={r.role} className="border-t border-[color:var(--color-edge)]">
                <td className="py-1 pr-2">{r.role}</td>
                <td className="py-1 pr-2">{r.turns}</td>
                <td className="py-1 pr-2">
                  {r.avgLatencyMs != null ? fmtMs(r.avgLatencyMs) : "—"}
                </td>
                <td className="py-1">
                  {r.tokensIn.toLocaleString()} in / {r.tokensOut.toLocaleString()} out
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {report.timeSavedMs != null && (
        <div className="pt-1 text-[color:var(--color-muted)]">
          {t("settings.triad.reportTimeSaved")}:{" "}
          <span className="text-[color:var(--color-accent-2)]">{fmtMs(report.timeSavedMs)}</span>
          {report.titanBaselineMs == null && (
            <span className="ml-1 text-[10px]">
              ({t("settings.triad.reportBaselineFallback")})
            </span>
          )}
        </div>
      )}
    </div>
  );
}

/** Compact durations: sub-second in ms, otherwise seconds. */
function fmtMs(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  return `${(ms / 1000).toLocaleString(undefined, { maximumFractionDigits: 1 })} s`;
}

/** Endpoint profiles (§5.1): CRUD + connection test. Changes persist
 * immediately — the bottom Save bar only covers the settings keys. */
function EndpointsTab({
  profiles,
  onReload,
}: {
  profiles: EndpointProfile[];
  onReload: () => Promise<void>;
}) {
  const [addOpen, setAddOpen] = useState(false);
  const [newKind, setNewKind] = useState("openai");
  const [newName, setNewName] = useState("");
  const [newBaseUrl, setNewBaseUrl] = useState("");
  const [newApiKey, setNewApiKey] = useState("");
  const [testResults, setTestResults] = useState<Record<string, EndpointTestResult>>({});
  const [testing, setTesting] = useState<Record<string, boolean>>({});
  const [keyDraft, setKeyDraft] = useState<Record<string, string>>({});

  const runTest = (p: EndpointProfile) => {
    setTesting((s) => ({ ...s, [p.id]: true }));
    void testEndpoint({ kind: p.kind, baseUrl: p.baseUrl, endpointId: p.id })
      .then((result) => setTestResults((s) => ({ ...s, [p.id]: result })))
      .catch(
        () =>
          setTestResults((s) => ({
            ...s,
            [p.id]: { ok: false, latencyMs: 0, modelCount: 0, models: [], error: "unreachable" },
          })),
      )
      .finally(() => setTesting((s) => ({ ...s, [p.id]: false })));
  };

  const addProfile = async () => {
    const saved = await saveEndpointProfile({
      id: "",
      kind: newKind,
      name: newName.trim(),
      baseUrl: newBaseUrl.trim(),
      apiKeyRef: null,
      headers: {},
      enabled: true,
      notes: null,
    }).catch(() => null);
    if (saved && newApiKey.trim()) {
      await setEndpointApiKey(saved.id, newApiKey.trim()).catch(() => {});
    }
    setNewName("");
    setNewBaseUrl("");
    setNewApiKey("");
    setAddOpen(false);
    await onReload();
  };

  const toggleEnabled = async (p: EndpointProfile) => {
    await saveEndpointProfile({ ...p, enabled: !p.enabled }).catch(() => {});
    await onReload();
  };

  const applyKey = async (p: EndpointProfile) => {
    const secret = keyDraft[p.id]?.trim();
    if (secret) {
      await setEndpointApiKey(p.id, secret).catch(() => {});
    }
    setKeyDraft((s) => ({ ...s, [p.id]: "" }));
    await onReload();
  };

  const dropProfile = async (p: EndpointProfile) => {
    await deleteEndpointProfile(p.id).catch(() => {});
    await onReload();
  };

  const BUILTIN = "ep_local_ollama";

  return (
    <div className="space-y-3">
      <div className="text-xs font-medium text-[color:var(--color-muted)]">
        {t("settings.endpoints.title")}
      </div>

      <div className="divide-y divide-[color:var(--color-edge)] rounded-lg border border-[color:var(--color-edge)]">
        {profiles.map((p) => {
          const result = testResults[p.id];
          const busy = testing[p.id];
          return (
            <div key={p.id} className="space-y-1.5 px-3 py-2.5">
              <div className="flex items-center justify-between gap-2">
                <div className="min-w-0">
                  <div className="flex items-center gap-2 text-sm text-[color:var(--color-ink)]">
                    <span className="truncate">{p.name}</span>
                    <span className="rounded bg-[color:var(--color-bg)] px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-[color:var(--color-muted)]">
                      {p.kind === "openai" ? "OpenAI" : "Ollama"}
                    </span>
                    {p.id === BUILTIN && (
                      <span className="rounded bg-[color:var(--color-bg)] px-1.5 py-0.5 text-[10px] text-[color:var(--color-accent-2)]">
                        {t("settings.endpoints.builtin")}
                      </span>
                    )}
                  </div>
                  <div className="truncate font-mono text-xs text-[color:var(--color-muted)]">
                    {p.baseUrl}
                  </div>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  {!p.apiKeyRef && p.id !== BUILTIN && (
                    <span className="text-[10px] text-[color:var(--color-muted)]">
                      {t("settings.endpoints.apiKeyNone")}
                    </span>
                  )}
                  {p.apiKeyRef && (
                    <button
                      type="button"
                      onClick={() => {
                        void clearEndpointApiKey(p.id).then(() => onReload());
                      }}
                      title={t("settings.endpoints.clearKey")}
                      className="rounded bg-[color:var(--color-bg)] px-1.5 py-0.5 text-[10px] text-[color:var(--color-accent-2)] hover:opacity-80"
                    >
                      {t("settings.endpoints.apiKeySet")}
                    </button>
                  )}
                  {p.id !== BUILTIN && (
                    <button
                      type="button"
                      onClick={() => void toggleEnabled(p)}
                      className={`rounded px-1.5 py-0.5 text-[10px] ${
                        p.enabled
                          ? "text-[color:var(--color-accent-2)]"
                          : "text-[color:var(--color-muted)]"
                      }`}
                    >
                      {p.enabled ? "●" : "○"}
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={() => void runTest(p)}
                    className="rounded-lg border border-[color:var(--color-edge)] px-2 py-1 text-xs text-[color:var(--color-ink)] hover:border-[color:var(--color-accent)]"
                  >
                    {busy ? t("settings.endpoints.testing") : t("settings.endpoints.test")}
                  </button>
                  {p.id !== BUILTIN && (
                    <button
                      type="button"
                      onClick={() => void dropProfile(p)}
                      className="rounded-lg border border-[color:var(--color-edge)] px-2 py-1 text-xs text-[color:var(--color-muted)] hover:text-[color:var(--color-danger)]"
                    >
                      ✕
                    </button>
                  )}
                </div>
              </div>

              {/* API key entry (stored in Credential Manager, §10.1) */}
              {p.kind === "openai" && (
                <div className="flex items-center gap-2">
                  <input
                    type="password"
                    value={keyDraft[p.id] ?? ""}
                    onChange={(e) => setKeyDraft((s) => ({ ...s, [p.id]: e.target.value }))}
                    placeholder={t("settings.endpoints.apiKey")}
                    className="min-w-0 flex-1 rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-bg)] px-2 py-1 text-xs outline-none focus:border-[color:var(--color-accent)]"
                  />
                  <button
                    type="button"
                    onClick={() => void applyKey(p)}
                    className="shrink-0 rounded-lg border border-[color:var(--color-edge)] px-2 py-1 text-xs text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)]"
                  >
                    {t("settings.endpoints.setKey")}
                  </button>
                </div>
              )}

              {/* Test outcome + latency/model count (§5.1) */}
              <div className="min-h-4 text-xs">
                {result?.ok && (
                  <span className="text-[color:var(--color-accent-2)]">
                    ✓ {result.modelCount} {t("settings.endpoints.models")} ·{" "}
                    {result.latencyMs} ms
                  </span>
                )}
                {result && !result.ok && (
                  <span className="break-all text-[color:var(--color-danger)]">
                    ✕ {result.error}
                  </span>
                )}
              </div>
            </div>
          );
        })}
      </div>

      {addOpen ? (
        <div className="space-y-2 rounded-lg border border-[color:var(--color-edge)] p-3">
          <div className="grid grid-cols-2 gap-2">
            <label className="block">
              <span className="mb-1 block text-[11px] font-medium text-[color:var(--color-muted)]">
                {t("settings.endpoints.kind")}
              </span>
              <select
                value={newKind}
                onChange={(e) => setNewKind(e.target.value)}
                className="w-full rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-bg)] px-2 py-1.5 text-sm outline-none focus:border-[color:var(--color-accent)]"
              >
                <option value="openai">{t("settings.endpoints.kindOpenai")}</option>
                <option value="ollama">{t("settings.endpoints.kindOllama")}</option>
              </select>
            </label>
            <label className="block">
              <span className="mb-1 block text-[11px] font-medium text-[color:var(--color-muted)]">
                {t("settings.endpoints.name")}
              </span>
              <input
                type="text"
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                placeholder="Homelab LM Studio"
                className="w-full rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-bg)] px-2.5 py-1.5 text-sm outline-none focus:border-[color:var(--color-accent)]"
              />
            </label>
          </div>
          <input
            type="text"
            value={newBaseUrl}
            onChange={(e) => setNewBaseUrl(e.target.value)}
            placeholder={newKind === "openai" ? "http://192.168.1.20:1234/v1" : "http://192.168.1.30:11434"}
            className="w-full rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-bg)] px-2.5 py-1.5 font-mono text-sm outline-none focus:border-[color:var(--color-accent)]"
          />
          {newKind === "openai" && (
            <input
              type="password"
              value={newApiKey}
              onChange={(e) => setNewApiKey(e.target.value)}
              placeholder={t("settings.endpoints.apiKey")}
              className="w-full rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-bg)] px-2.5 py-1.5 text-sm outline-none focus:border-[color:var(--color-accent)]"
            />
          )}
          <div className="flex justify-end gap-2">
            <button
              type="button"
              onClick={() => setAddOpen(false)}
              className="rounded-lg border border-[color:var(--color-edge)] px-3 py-1.5 text-xs text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)]"
            >
              {t("settings.close")}
            </button>
            <button
              type="button"
              onClick={() => void addProfile()}
              disabled={!newName.trim() || !newBaseUrl.trim()}
              className="rounded-lg bg-[color:var(--color-accent)] px-3 py-1.5 text-xs font-medium text-white hover:opacity-90 disabled:opacity-40"
            >
              {t("settings.endpoints.add")}
            </button>
          </div>
        </div>
      ) : (
        <button
          type="button"
          onClick={() => setAddOpen(true)}
          className="rounded-lg border border-dashed border-[color:var(--color-edge)] px-3 py-1.5 text-xs text-[color:var(--color-muted)] hover:border-[color:var(--color-accent)] hover:text-[color:var(--color-ink)]"
        >
          + {t("settings.endpoints.add")}
        </button>
      )}
    </div>
  );
}

/** Capability registry (§5.4): discovery facts + user records, grouped by
 * endpoint. Chips toggle vision/tools/thinking, Save persists the record
 * (verified by the act of editing), Re-detect re-probes via /api/show. */
function ModelsTab({ profiles }: { profiles: EndpointProfile[] }) {
  const [rows, setRows] = useState<ModelInfo[]>([]);
  const [drafts, setDrafts] = useState<Record<string, ModelDraft>>({});
  const [busy, setBusy] = useState<Record<string, string>>({});

  useEffect(() => {
    void reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const reload = async () => {
    const list = await listModels().catch(() => []);
    setRows(list);
    setDrafts(Object.fromEntries(list.map((m) => [keyOf(m), draftOf(m)])));
  };

  const kindOf = (endpointId: string): string =>
    profiles.find((p) => p.id === endpointId)?.kind ?? "ollama";

  const save = async (m: ModelInfo) => {
    const k = keyOf(m);
    const draft = drafts[k];
    if (!draft) return;
    setBusy((s) => ({ ...s, [k]: "save" }));
    await saveModelRecord({
      endpointId: m.endpointId,
      model: bareModel(m.id),
      capabilities: draft.capabilities,
      contextTokens: parseTokens(draft.contextTokens),
    }).catch(() => {});
    setBusy((s) => ({ ...s, [k]: "" }));
    await reload();
  };

  const clear = async (m: ModelInfo) => {
    const k = keyOf(m);
    setBusy((s) => ({ ...s, [k]: "clear" }));
    await clearModelRecord(m.endpointId, bareModel(m.id)).catch(() => {});
    setBusy((s) => ({ ...s, [k]: "" }));
    await reload();
  };

  const redetect = async (m: ModelInfo) => {
    const k = keyOf(m);
    setBusy((s) => ({ ...s, [k]: "verify" }));
    await verifyModel(m.endpointId, bareModel(m.id)).catch(() => {});
    setBusy((s) => ({ ...s, [k]: "" }));
    await reload();
  };

  // Group in endpoint order (built-in first, then profiles).
  const orderedEndpoints = [
    ...profiles.map((p) => p.id),
    ...rows.map((m) => m.endpointId),
  ].filter((id, i, arr) => arr.indexOf(id) === i);

  return (
    <div className="space-y-3">
      <div className="text-xs font-medium text-[color:var(--color-muted)]">
        {t("settings.models.title")}
      </div>

      {rows.length === 0 && (
        <div className="rounded-lg border border-[color:var(--color-edge)] px-3 py-4 text-center text-xs text-[color:var(--color-muted)]">
          {t("settings.models.none")}
        </div>
      )}

      <div className="divide-y divide-[color:var(--color-edge)] rounded-lg border border-[color:var(--color-edge)]">
        {orderedEndpoints.map((endpointId) => {
          const endpointRows = rows.filter((m) => m.endpointId === endpointId);
          if (endpointRows.length === 0) return null;
          const profile = profiles.find((p) => p.id === endpointId);
          return (
            <div key={endpointId} className="px-3 py-2.5">
              <div className="mb-1.5 flex items-center gap-2 text-[11px] font-medium text-[color:var(--color-muted)]">
                <span>{profile?.name ?? endpointId}</span>
                {endpointId === "ep_local_ollama" && (
                  <span className="rounded bg-[color:var(--color-bg)] px-1.5 py-0.5 text-[10px] text-[color:var(--color-accent-2)]">
                    {t("settings.endpoints.builtin")}
                  </span>
                )}
              </div>
              <div className="space-y-1">
                {endpointRows.map((m) => {
                  const k = keyOf(m);
                  const draft = drafts[k];
                  return (
                    <div
                      key={m.id}
                      className="flex flex-wrap items-center gap-2 rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-bg)] px-2.5 py-1.5"
                    >
                      <span className="min-w-0 flex-1 truncate font-mono text-xs text-[color:var(--color-ink)]">
                        {m.id}
                      </span>
                      {CAP_TAGS.map((tag) => {
                        const on = draft?.capabilities.includes(tag) ?? false;
                        return (
                          <button
                            key={tag}
                            type="button"
                            onClick={() =>
                              setDrafts((s) => {
                                const d = s[k] ?? draftOf(m);
                                return {
                                  ...s,
                                  [k]: {
                                    ...d,
                                    capabilities: on
                                      ? d.capabilities.filter((c) => c !== tag)
                                      : [...d.capabilities, tag],
                                  },
                                };
                              })
                            }
                            className={`rounded px-1.5 py-0.5 text-[10px] ${
                              on
                                ? "bg-[color:var(--color-accent)] text-white"
                                : "border border-[color:var(--color-edge)] text-[color:var(--color-muted)] hover:border-[color:var(--color-accent)]"
                            }`}
                          >
                            {tag}
                          </button>
                        );
                      })}
                      <input
                        type="text"
                        value={draft?.contextTokens ?? ""}
                        onChange={(e) =>
                          setDrafts((s) => ({
                            ...s,
                            [k]: { ...s[k], contextTokens: e.target.value },
                          }))
                        }
                        placeholder={t("settings.models.contextTokens")}
                        className="w-24 rounded border border-[color:var(--color-edge)] px-2 py-1 text-xs outline-none focus:border-[color:var(--color-accent)]"
                      />
                      <button
                        type="button"
                        onClick={() => void save(m)}
                        className="rounded-lg bg-[color:var(--color-accent)] px-2 py-1 text-[10px] font-medium text-white hover:opacity-90"
                      >
                        {busy[k] === "save" ? "…" : t("settings.models.save")}
                      </button>
                      {kindOf(endpointId) === "ollama" && (
                        <button
                          type="button"
                          onClick={() => void redetect(m)}
                          title={t("settings.models.redetectHint")}
                          className="rounded-lg border border-[color:var(--color-edge)] px-2 py-1 text-[10px] text-[color:var(--color-muted)] hover:border-[color:var(--color-accent)] hover:text-[color:var(--color-ink)]"
                        >
                          {busy[k] === "verify" ? "…" : t("settings.models.redetect")}
                        </button>
                      )}
                      <button
                        type="button"
                        onClick={() => void clear(m)}
                        title={t("settings.models.clearHint")}
                        className="rounded-lg border border-[color:var(--color-edge)] px-2 py-1 text-[10px] text-[color:var(--color-muted)] hover:text-[color:var(--color-danger)]"
                      >
                        {t("settings.models.clear")}
                      </button>
                    </div>
                  );
                })}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

const CAP_TAGS = ["vision", "tools", "thinking"] as const;

interface ModelDraft {
  capabilities: string[];
  contextTokens: string;
}

function keyOf(m: ModelInfo): string {
  return `${m.endpointId}|${m.id}`;
}

/** Draft keeps every tag (completion + custom) so Save never drops facts it
 * didn't show a chip for. */
function draftOf(m: ModelInfo): ModelDraft {
  return {
    capabilities: [...m.capabilities],
    contextTokens: m.contextLength != null ? String(m.contextLength) : "",
  };
}

/** "qwen2.5vl:7b@ep_remote" → "qwen2.5vl:7b" (records are keyed by bare name). */
function bareModel(id: string): string {
  const at = id.lastIndexOf("@");
  return at === -1 ? id : id.slice(0, at);
}

function parseTokens(raw: string): number | null {
  const n = Number.parseInt(raw.trim(), 10);
  return Number.isFinite(n) && n > 0 ? n : null;
}

function RoleSelect({
  label,
  models,
  value,
  onChange,
}: {
  label: string;
  models: { id: string }[];
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <div>
      <label className="mb-1 block text-[11px] font-medium text-[color:var(--color-muted)]">
        {label}
      </label>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="w-full rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-bg)] px-2 py-1.5 text-sm outline-none focus:border-[color:var(--color-accent)]"
      >
        <option value="">{t("settings.triad.unassigned")}</option>
        {models.map((m) => (
          <option key={m.id} value={m.id}>
            {m.id}
          </option>
        ))}
      </select>
    </div>
  );
}

function Field({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <div>
      <label className="mb-1 block text-xs font-medium text-[color:var(--color-muted)]">
        {label}
      </label>
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="w-full rounded-lg border border-[color:var(--color-edge)] bg-[color:var(--color-bg)] px-2.5 py-1.5 text-sm outline-none focus:border-[color:var(--color-accent)]"
      />
    </div>
  );
}