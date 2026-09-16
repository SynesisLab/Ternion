import { useEffect, useState } from "react";

import { getSetting, listModels, setSetting } from "../lib/ipc";
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
type Tab = "general" | "triad";

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

  useEffect(() => {
    if (!open) return;
    void (async () => {
      const [url, temp, ctx, ka, tray] = await Promise.all([
        getSetting(settingsKeys.ollamaBaseUrl),
        getSetting(settingsKeys.chatTemperature),
        getSetting(settingsKeys.chatContextTokens),
        getSetting(settingsKeys.chatKeepAlive),
        getSetting(settingsKeys.appCloseToTray),
      ]);
      setBaseUrl(url ?? DEFAULT_BASE_URL);
      setTemperature(temp ?? DEFAULT_TEMPERATURE);
      setContextTokens(ctx ?? DEFAULT_CONTEXT_TOKENS);
      setKeepAlive(ka ?? DEFAULT_KEEP_ALIVE);
      setCloseToTray((tray ?? "true") !== "false");

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
            <TabButton active={tab === "triad"} onClick={() => setTab("triad")}>
              {t("settings.tab.triad")}
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
          </div>
        ) : (
          <div className="space-y-4">
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