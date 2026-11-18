import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl as tauriOpenUrl } from "@tauri-apps/plugin-opener";

export { invoke, listen };
export type { UnlistenFn };

export const isWeb = false;

export async function hideWindow(): Promise<void> {
  await getCurrentWindow().hide();
}

export async function unmaximizeWindow(): Promise<void> {
  await getCurrentWindow().unmaximize();
}

export async function openUrl(url: string): Promise<void> {
  await tauriOpenUrl(url);
}

export async function uploadBackground(_image: Blob): Promise<{ url: string }> {
  throw new Error("自定义背景仅 Web/Linux 版支持");
}

export async function clearBackground(): Promise<void> {
  throw new Error("自定义背景仅 Web/Linux 版支持");
}
