import { useEffect, useState } from "react";

import { getSetting, listModels, setSetting } from "../lib/ipc";
import { settingsKeys } from "../lib/settingsKeys";
import { t } from "../i18n";
import { useChatStore } from "../store/chatStore";

const DEFAULT_BASE_URL = "http://127.0.0.1:11434";
const DEFAULT_TEMPERATURE = "0.7";
const DEFAULT_CONTEXT_TOKENS = "8192";
const DEFAULT_KEEP_ALIVE = "10m";

type TestState = "idle" | "testing" | "ok" | "fail";

export function SettingsDialog({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const refreshModels = useChatStore((s) => s.refreshModels);

  const [baseUrl, setBaseUrl] = useState(DEFAULT_BASE_URL);
  const [temperature, setTemperature] = useState(DEFAULT_TEMPERATURE);
  const [contextTokens, setContextTokens] = useState(DEFAULT_CONTEXT_TOKENS);
  const [keepAlive, setKeepAlive] = useState(DEFAULT_KEEP_ALIVE);
  const [closeToTray, setCloseToTray] = useState(true);
  const [test, setTest] = useState<TestState>("idle");
  const [saved, setSaved] = useState(false);

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
    })();
  }, [open]);

  if (!open) return null;

  const resetDefaults = () => {
    setBaseUrl(DEFAULT_BASE_URL);
    setTemperature(DEFAULT_TEMPERATURE);
    setContextTokens(DEFAULT_CONTEXT_TOKENS);
    setKeepAlive(DEFAULT_KEEP_ALIVE);
    setCloseToTray(true);
  };

  const runTest = async () => {
    setTest("testing");
    try {
      const models = await listModels();
      setTest(models.length > 0 ? "ok" : "fail");
    } catch {
      setTest("fail");
    }
  };

  const save = async () => {
    const url = baseUrl.trim().replace(/\/+$/, "");
    const writes: Array<Promise<unknown>> = [
      setSetting(settingsKeys.ollamaBaseUrl, url),
      setSetting(settingsKeys.chatTemperature, temperature.trim() || DEFAULT_TEMPERATURE),
      setSetting(
        settingsKeys.chatContextTokens,
        contextTokens.trim() || DEFAULT_CONTEXT_TOKENS,
      ),
      setSetting(settingsKeys.chatKeepAlive, keepAlive.trim() || DEFAULT_KEEP_ALIVE),
      setSetting(settingsKeys.appCloseToTray, String(closeToTray)),
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
      <div className="w-full max-w-md rounded-xl border border-[color:var(--color-edge)] bg-[color:var(--color-panel)] p-5 shadow-2xl">
        <div className="mb-4 flex items-center justify-between">
          <div className="text-sm font-semibold">{t("settings.title")}</div>
          <button
            type="button"
            onClick={onClose}
            className="rounded px-2 text-sm text-[color:var(--color-muted)] hover:text-[color:var(--color-ink)]"
          >
            ✕
          </button>
        </div>

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