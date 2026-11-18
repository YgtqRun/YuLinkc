const API_BASE = import.meta.env.BASE_URL;

type JsonRecord = Record<string, unknown>;

async function request<T>(
  method: "GET" | "POST",
  path: string,
  body?: unknown
): Promise<T> {
  const response = await fetch(`${API_BASE}api/${path}`, {
    method,
    cache: "no-store",
    headers: body === undefined ? undefined : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  let data: unknown = text;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      // Keep raw text for non-JSON error pages.
    }
  }
  if (!response.ok) {
    const message =
      typeof data === "string"
        ? data
        : (data as JsonRecord | null)?.error ?? `HTTP ${response.status}`;
    throw new Error(String(message));
  }
  return data as T;
}

export async function invoke<T>(
  cmd: string,
  args?: Record<string, unknown>
): Promise<T> {
  switch (cmd) {
    case "get_settings":
      return request<T>("GET", "settings");
    case "save_settings":
      return request<T>("POST", "settings", args?.req ?? {});
    case "clear_sms_code":
      return request<T>("POST", "sms/clear");
    case "clear_all":
      return request<T>("POST", "clear-all");
    case "login_now":
      return request<T>("POST", "login");
    case "logout_now":
      return request<T>("POST", "logout");
    case "get_runtime_status":
      return request<T>("GET", "status");
    case "get_app_info":
      return request<T>("GET", "app-info");
    case "set_autostart":
      return { ok: false, message: "Web 版由 NAS 常驻服务管理" } as T;
    case "check_update":
      return {
        current: "",
        latest: null,
        hasUpdate: false,
        url: null,
        notes: null,
        error: "Web 版暂不支持在线更新",
      } as T;
    default:
      throw new Error(`Web 版不支持命令: ${cmd}`);
  }
}

export type UnlistenFn = () => void;

export async function listen<T>(
  event: string,
  handler: (event: { payload: T }) => void
): Promise<UnlistenFn> {
  if (event !== "yulink://status") {
    return () => {};
  }
  let disposed = false;
  let timer: number | undefined;
  let lastStatus = "";
  const tick = async () => {
    if (disposed) return;
    try {
      const payload = await invoke<T>("get_runtime_status");
      const serialized = JSON.stringify(payload);
      if (!disposed && serialized !== lastStatus) {
        lastStatus = serialized;
        handler({ payload });
      }
    } catch {
      // Keep polling; the API may be restarting.
    }
    if (!disposed) {
      timer = window.setTimeout(tick, 3000);
    }
  };
  void tick();
  return () => {
    disposed = true;
    if (timer !== undefined) {
      window.clearTimeout(timer);
    }
  };
}

export const isWeb = true;

export async function hideWindow(): Promise<void> {
  // The web shell has no native window to hide.
}

export async function unmaximizeWindow(): Promise<void> {
  // The web shell is not maximizable.
}

export async function openUrl(url: string): Promise<void> {
  window.open(url, "_blank", "noopener,noreferrer");
}

export async function uploadBackground(image: Blob): Promise<{ url: string }> {
  const response = await fetch(`${API_BASE}api/background`, {
    method: "POST",
    cache: "no-store",
    headers: { "Content-Type": image.type || "application/octet-stream" },
    body: image,
  });
  if (!response.ok) {
    throw new Error((await response.text()) || `HTTP ${response.status}`);
  }
  return response.json() as Promise<{ url: string }>;
}

export async function clearBackground(): Promise<void> {
  const response = await fetch(`${API_BASE}api/background/clear`, {
    method: "POST",
    cache: "no-store",
  });
  if (!response.ok) {
    throw new Error((await response.text()) || `HTTP ${response.status}`);
  }
}
