// 桌面版用到的一小撮 Tauri API 在 Web 环境下的替身。
//
// 目标只是让组件里的调用点**零改动**：import 换成本模块即可。
// 每个替身都尽量保持原语义，实在没有对应概念的（例如「在文件管理器里
// 打开目录」）就退化成安全的无操作，而不是抛错打断用户。

/* eslint-disable */
// @ts-nocheck

import { APP_VERSION } from "./bindings.ts";

/**
 * `@tauri-apps/api` 的 `path`。
 *
 * 浏览器里没有真正的路径概念，但服务端配置里的目录是 POSIX 路径，
 * 而桌面版调用 `path.join` 的唯一用途就是拼出给服务端看的路径，
 * 所以一个朴素的、与平台无关的 join 就够用。
 */
export const path = {
  join(...segments: string[]): string {
    const cleaned = segments
      .filter((s) => s !== undefined && s !== null && s !== "")
      .map((s) => s.replace(/\/+$/, ""));
    if (cleaned.length === 0) return "";
    const [head, ...rest] = cleaned;
    const tail = rest
      .map((s) => s.replace(/^\/+/, ""))
      .filter((s) => s !== "")
      .join("/");
    if (!tail) return head;
    return head.endsWith("/") ? `${head}${tail}` : `${head}/${tail}`;
  },
};

/**
 * `@tauri-apps/api/app` 的 `getVersion`。
 *
 * Web 版没有安装包版本可读，用编译期常量代替。
 */
export async function getVersion(): Promise<string> {
  return APP_VERSION;
}

/**
 * `@tauri-apps/plugin-opener` 的 `openUrl`。
 *
 * 浏览器里就是开个新标签页。
 */
export async function openUrl(url: string): Promise<void> {
  window.open(url, "_blank", "noopener,noreferrer");
}

/**
 * `@tauri-apps/api/path` 的 `appDataDir`。
 *
 * 桌面版用它定位配置/日志目录。Web 版这些目录在**服务端**，
 * 浏览器无从得知，所以返回服务端在下发的 `ServerInfo` 里给的提示值。
 * 调用方只会把它拼进路径再交给服务端，因此占位符语义上够用。
 */
export async function appDataDir(): Promise<string> {
  return "/data";
}

/**
 * `@tauri-apps/plugin-dialog` 的 `open`。
 *
 * 桌面版用它弹原生目录选择器。浏览器出于安全沙箱**拿不到真实路径**
 * （`<input type=file>` 只给文件名，`showDirectoryPicker` 给的也是句柄
 * 而不是服务端可用的路径）。
 *
 * 因此这里退化为 `window.prompt`：让用户直接输入服务端路径。
 * 虽然不如原生选择器顺手，但语义正确 —— 目录本来就属于服务端。
 */
export async function open(options?: { directory?: boolean }): Promise<string | null> {
  const label = options?.directory ? "请输入目录路径" : "请输入文件路径";
  const value = window.prompt(label);
  if (value === null) return null; // 用户取消
  const trimmed = value.trim();
  return trimmed === "" ? null : trimmed;
}
