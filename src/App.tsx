import { useEffect, useState } from "react";
import { ping } from "./lib/ipc";

export default function App() {
  const [version, setVersion] = useState<string | null>(null);
  const [ipcError, setIpcError] = useState<string | null>(null);

  useEffect(() => {
    ping()
      .then(setVersion)
      .catch((e) => setIpcError(String(e)));
  }, []);

  return (
    <div className="flex h-screen items-center justify-center bg-[#0b0e14] text-slate-200">
      <div className="text-center">
        <div className="text-4xl font-semibold tracking-tight">Ternion</div>
        {version !== null && (
          <div className="mt-2 text-sm text-slate-500">core v{version} — IPC ✓</div>
        )}
        {ipcError !== null && (
          <div className="mt-2 text-sm text-[color:var(--color-danger)]">
            IPC failed: {ipcError}
          </div>
        )}
      </div>
    </div>
  );
}