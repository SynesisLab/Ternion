import { invoke } from "@tauri-apps/api/core";

/**
 * Typed wrappers around Tauri IPC commands. Every command the Rust core
 * exposes gets one function here; the frontend never calls `invoke`
 * directly elsewhere.
 */

export async function ping(): Promise<string> {
  return invoke<string>("ping");
}