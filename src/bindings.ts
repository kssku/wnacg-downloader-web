// 手写的 Web 数据层，替代原 tauri-specta 生成的 bindings.ts。
//
// 保留 `commands.xxx(...)` 与 `events.yyy.listen(cb)` 的调用形状，只替换传输层：
//   - commands -> HTTP，GET 走查询串、POST 走 JSON body（错误体为 CommandError）
//   - events   -> 单条 WebSocket 连接，按 topic 分发
//
// 这样所有组件都不需要改动。

/* eslint-disable */
// @ts-nocheck

/** ============================ 基础配置 ============================ */

/** 后端 base url。生产态与前端同源（由 axum 托管静态资源），开发态走 vite 代理。 */
const BASE_URL: string = "";

/** 认证 token 的 localStorage key。 */
const TOKEN_KEY = "wnacg_token";

function getToken(): string {
  return localStorage.getItem(TOKEN_KEY) ?? "";
}

export function setToken(token: string): void {
  if (token) localStorage.setItem(TOKEN_KEY, token);
  else localStorage.removeItem(TOKEN_KEY);
}

export function clearToken(): void {
  localStorage.removeItem(TOKEN_KEY);
}

/** ============================ 类型定义 ============================ */

export type Result<T, E> =
  | { status: "ok"; data: T }
  | { status: "error"; error: E };

export type CommandError = { err_title: string; err_message: string };

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | Partial<{ [key in string]: JsonValue }>;

export type ApiDomainMode = "Default" | "Custom";
export type DownloadFormat = "Jpeg" | "Png" | "Webp" | "Original";
export type ProxyMode = "System" | "NoProxy" | "Custom";
export type LogLevel = "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR";
export type DownloadTaskState =
  | "Pending"
  | "Downloading"
  | "Paused"
  | "Cancelled"
  | "Completed"
  | "Failed";

export type Config = {
  cookie: string;
  downloadDir: string;
  exportDir: string;
  enableFileLogger: boolean;
  downloadFormat: DownloadFormat;
  proxyMode: ProxyMode;
  proxyHost: string;
  proxyPort: number;
  comicConcurrency: number;
  comicDownloadIntervalSec: number;
  imgConcurrency: number;
  imgDownloadIntervalSec: number;
  downloadShelfIntervalMs: number;
  batchDownloadIntervalMs: number;
  useOriginalFilename: boolean;
  apiDomainMode: ApiDomainMode;
  customApiDomain: string;
};

export type Tag = { name: string; url: string };
export type ImgInImgList = { caption: string; url: string };
export type ImgList = ImgInImgList[];

export type Comic = {
  id: number;
  title: string;
  cover: string;
  category: string;
  imageCount: number;
  tags: Tag[];
  intro: string;
  isDownloaded?: boolean | null;
  imgList: ImgList;
};

export type ComicInSearch = {
  id: number;
  titleHtml: string;
  title: string;
  cover: string;
  additionalInfo: string;
  isDownloaded: boolean;
};

export type Shelf = { id: number; name: string };

export type ComicInShelf = {
  id: number;
  title: string;
  cover: string;
  favoriteTime: string;
  shelf: Shelf;
  isDownloaded: boolean;
};

export type SearchResult = {
  comics: ComicInSearch[];
  currentPage: number;
  totalPage: number;
  isSearchByTag: boolean;
};

export type GetShelfResult = {
  comics: ComicInShelf[];
  currentPage: number;
  totalPage: number;
  shelf: Shelf;
  shelves: Shelf[];
};

export type UserProfile = { username: string; avatar: string };

export type LogEvent = {
  timestamp: string;
  level: LogLevel;
  fields: Partial<{ [key in string]: JsonValue }>;
  target: string;
  filename: string;
  line_number: number;
};

export type DownloadTaskEvent = {
  state: DownloadTaskState;
  comic: Comic;
  downloadedImgCount: number;
  totalImgCount: number;
};

/** 任务被删除时后端单独发的信号。 */
export type DownloadTaskDeletedEvent = { comicId: number };

export type DownloadSpeedEvent = { speed: string };
export type DownloadSleepingEvent = { comicId: number; remainingSec: number };

export type DownloadShelfEvent =
  | { event: "GettingShelfComics" }
  | { event: "CreatingDownloadTask"; data: { current: number; total: number } }
  | { event: "End" };

export type ExportPdfEvent =
  | { event: "Start"; data: { uuid: string; title: string } }
  | { event: "End"; data: { uuid: string } };

export type ExportCbzEvent =
  | { event: "Start"; data: { uuid: string; title: string } }
  | { event: "End"; data: { uuid: string } };

export type ServerInfo = {
  version: string;
  dataDir: string;
  downloadDir: string;
  eventSubscribers: number;
};

/* ============================ HTTP 传输层 ============================ */

/** 从任意失败响应里尽力还原 CommandError。 */
async function toCommandError(res: Response): Promise<CommandError> {
  let title = `HTTP ${res.status}`;
  let message = res.statusText || "请求失败";
  try {
    const body = await res.json();
    if (body && (body.err_title || body.err_message)) {
      title = body.err_title ?? title;
      message = body.err_message ?? message;
    } else if (body && (body.errTitle || body.errMessage)) {
      // 鉴权中间件用 camelCase
      title = body.errTitle ?? title;
      message = body.errMessage ?? message;
    }
  } catch {
    // 非 JSON 响应，保留默认文案
  }
  return { err_title: title, err_message: message };
}

function authHeaders(): Record<string, string> {
  const token = getToken();
  const headers: Record<string, string> = {};
  if (token) headers["Authorization"] = `Bearer ${token}`;
  return headers;
}

/** POST /api/... ，body 为 JSON。 */
async function post(path: string, body?: unknown): Promise<Response> {
  return await fetch(`${BASE_URL}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", ...authHeaders() },
    body: JSON.stringify(body ?? {}),
  });
}

/** GET /api/...?k=v，自动跳过 undefined/null。 */
async function get(path: string, params?: Record<string, unknown>): Promise<Response> {
  const query = new URLSearchParams();
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      if (v !== undefined && v !== null) query.set(k, String(v));
    }
  }
  const qs = query.toString();
  return await fetch(`${BASE_URL}${path}${qs ? `?${qs}` : ""}`, {
    method: "GET",
    headers: authHeaders(),
  });
}

/**
 * 包装成 `Result<T, CommandError>`。
 * 网络层异常（断网 / 后端没起）也转成 CommandError 而不是抛出，
 * 这样调用方原有的 `if (result.status === "error")` 分支依然有效。
 */
async function callResult<T>(fn: () => Promise<Response>): Promise<Result<T, CommandError>> {
  try {
    const res = await fn();
    if (!res.ok) return { status: "error", error: await toCommandError(res) };
    const text = await res.text();
    const data = text ? (JSON.parse(text) as T) : (null as T);
    return { status: "ok", data };
  } catch (e) {
    return {
      status: "error",
      error: {
        err_title: "网络错误",
        err_message: e instanceof Error ? e.message : String(e),
      },
    };
  }
}

/** 直返版本，失败时抛出。对应原 `Promise<T>`（非 Result）的命令。 */
async function callDirect<T>(fn: () => Promise<Response>): Promise<T> {
  const res = await fn();
  if (!res.ok) {
    const err = await toCommandError(res);
    throw new Error(`${err.err_title}: ${err.err_message}`);
  }
  const text = await res.text();
  return text ? (JSON.parse(text) as T) : (null as T);
}

/* ============================ 命令层 ============================ */

export const commands = {
  /** POST /api/login —— 返回服务端 token。 */
  async login(username: string, password: string): Promise<Result<string, CommandError>> {
    return await callResult<string>(() => post("/api/login", { username, password }));
  },

  async getConfig(): Promise<Config> {
    return await callDirect<Config>(() => get("/api/config"));
  },

  async saveConfig(config: Config): Promise<Result<null, CommandError>> {
    return await callResult<null>(() => post("/api/config", { config }));
  },

  async getUserProfile(): Promise<Result<UserProfile, CommandError>> {
    return await callResult<UserProfile>(() => post("/api/user/profile", {}));
  },

  async searchByKeyword(keyword: string, pageNum: number): Promise<Result<SearchResult, CommandError>> {
    return await callResult<SearchResult>(() =>
      post("/api/search/keyword", { keyword, pageNum }),
    );
  },

  async searchByTag(tagName: string, pageNum: number): Promise<Result<SearchResult, CommandError>> {
    return await callResult<SearchResult>(() => post("/api/search/tag", { tagName, pageNum }));
  },

  async getComic(id: number): Promise<Result<Comic, CommandError>> {
    return await callResult<Comic>(() => get(`/api/comic/${id}`));
  },

  async getShelf(shelfId: number, pageNum: number): Promise<Result<GetShelfResult, CommandError>> {
    return await callResult<GetShelfResult>(() => post("/api/shelf", { shelfId, pageNum }));
  },

  async downloadShelf(shelfId: number): Promise<Result<null, CommandError>> {
    return await callResult<null>(() => post("/api/shelf/download", { shelfId }));
  },

  /** 直接创建下载任务，无返回值（与桌面版一致）。 */
  async createDownloadTask(comic: Comic): Promise<void> {
    await callDirect<null>(() => post("/api/download/task", { comic }));
  },

  async pauseDownloadTask(comicId: number): Promise<Result<null, CommandError>> {
    return await callResult<null>(() => post(`/api/download/task/${comicId}/pause`));
  },

  async resumeDownloadTask(comicId: number): Promise<Result<null, CommandError>> {
    return await callResult<null>(() => post(`/api/download/task/${comicId}/resume`));
  },

  async cancelDownloadTask(comicId: number): Promise<Result<null, CommandError>> {
    return await callResult<null>(() => post(`/api/download/task/${comicId}/cancel`));
  },

  async getDownloadedComics(): Promise<Result<Comic[], CommandError>> {
    return await callResult<Comic[]>(() => get("/api/downloaded/comics"));
  },

  async exportPdf(comic: Comic): Promise<Result<null, CommandError>> {
    return await callResult<null>(() => post("/api/export/pdf", { comic }));
  },

  async exportCbz(comic: Comic): Promise<Result<null, CommandError>> {
    return await callResult<null>(() => post("/api/export/cbz", { comic }));
  },

  async getLogsDirSize(): Promise<Result<number, CommandError>> {
    return await callResult<number>(() => post("/api/logs/size", {}));
  },

  /**
   * 桌面版是「在文件管理器里打开目录」，Web 版没有这个概念。
   * 保留签名让调用点不改动，实际是 no-op（成功返回），
   * 避免用户点一下按钮就弹错误。
   */
  async showPathInFileManager(_path: string): Promise<Result<null, CommandError>> {
    return { status: "ok", data: null };
  },

  /**
   * 桌面版返回 `number[]`（IPC 只能传数组），Web 版同样返回
   * `number[]`（内部走 fetch + arrayBuffer），这样调用方无需改动。
   */
  async getCoverData(coverUrl: string): Promise<Result<number[], CommandError>> {
    return await callResult<number[]>(async () => {
      return await fetch(
        `${BASE_URL}/api/cover?cover_url=${encodeURIComponent(coverUrl)}`,
        { method: "GET", headers: authHeaders() },
      );
    });
  },
};

/* ============================ WebSocket 事件层 ============================ */

type Listener = (payload: any) => void;

const listeners = new Map<string, Set<Listener>>();
const lastPayload = new Map<string, any>();

let socket: WebSocket | null = null;
let reconnectDelay = 1000;
let heartbeatTimer: number | null = null;
let manualClose = false;

function wsUrl(token: string): string {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  // 浏览器 WebSocket 构造函数不支持自定义请求头，凭证只能走查询串。
  // 后端 `require_auth` 有对应的 `?token=` 兜底分支。
  const query = token ? `?token=${encodeURIComponent(token)}` : "";
  return `${proto}//${location.host}/api/ws${query}`;
}

function dispatch(topic: string, payload: any): void {
  lastPayload.set(topic, payload);
  const set = listeners.get(topic);
  if (!set) return;
  for (const fn of set) {
    try {
      fn(payload);
    } catch (e) {
      console.error(`[ws] listener for "${topic}" threw`, e);
    }
  }
}

function scheduleReconnect(): void {
  if (manualClose) return;
  window.setTimeout(() => connect(), reconnectDelay);
  // 指数退避，上限 15s
  reconnectDelay = Math.min(reconnectDelay * 2, 15000);
}

function connect(): void {
  if (
    socket &&
    (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)
  ) {
    return;
  }
  // token 可能为空（后端关闭了认证），此时也建立连接，只是不带凭证。
  socket = new WebSocket(wsUrl(getToken()));

  socket.onopen = () => {
    reconnectDelay = 1000;
    if (heartbeatTimer !== null) window.clearInterval(heartbeatTimer);
    heartbeatTimer = window.setInterval(() => {
      if (socket?.readyState === WebSocket.OPEN) {
        try {
          socket.send("ping");
        } catch {
          /* ignore */
        }
      }
    }, 25000);
  };

  socket.onmessage = (ev: MessageEvent<string>) => {
    if (!ev.data || ev.data === "pong") return;
    let msg: { topic?: string; payload?: any };
    try {
      msg = JSON.parse(ev.data);
    } catch {
      return;
    }
    if (!msg.topic) return;
    dispatch(msg.topic, msg.payload);
  };

  socket.onclose = () => {
    if (heartbeatTimer !== null) {
      window.clearInterval(heartbeatTimer);
      heartbeatTimer = null;
    }
    socket = null;
    scheduleReconnect();
  };

  socket.onerror = () => {
    // onclose 紧随其后，统一在那里重连
  };
}

/** 登录成功后调用，建立（或重建）事件连接。 */
export function reconnectEvents(): void {
  manualClose = false;
  if (socket) {
    try {
      socket.close();
    } catch {
      /* ignore */
    }
    socket = null;
  }
  connect();
}

/** 登出时调用，断开并清空。 */
export function disconnectEvents(): void {
  manualClose = true;
  if (heartbeatTimer !== null) {
    window.clearInterval(heartbeatTimer);
    heartbeatTimer = null;
  }
  if (socket) {
    try {
      socket.close();
    } catch {
      /* ignore */
    }
    socket = null;
  }
  lastPayload.clear();
}

/** 前端事件名 -> 后端 topic 字符串（需与 src-server/src/event_bus.rs 的 topics 一致）。 */
const TOPIC_MAP: Record<string, string> = {
  downloadShelfEvent: "download-shelf-event",
  downloadSleepingEvent: "download-sleeping-event",
  downloadSpeedEvent: "download-speed-event",
  downloadTaskEvent: "download-task-event",
  downloadTaskDeletedEvent: "download-task-deleted-event",
  exportCbzEvent: "export-cbz-event",
  exportPdfEvent: "export-pdf-event",
  logEvent: "log-event",
  taskSnapshot: "task-snapshot-event",
};

function subscribe(key: string, cb: (ev: { payload: any }) => void): () => void {
  const topic = TOPIC_MAP[key];
  const wrapped: Listener = (payload) => cb({ payload });

  let set = listeners.get(topic);
  if (!set) {
    set = new Set();
    listeners.set(topic, set);
  }
  set.add(wrapped);

  // 新订阅者立刻拿到该 topic 最近一条消息，避免组件挂载晚于事件到达而漏掉状态
  // （尤其是 taskSnapshot —— WebSocket 建连时后端会推一次全量任务状态）。
  const cached = lastPayload.get(topic);
  if (cached !== undefined) {
    try {
      cb({ payload: cached });
    } catch (e) {
      console.error(`[ws] replay for "${topic}" threw`, e);
    }
  }

  connect();

  return () => {
    set?.delete(wrapped);
  };
}

/**
 * 保持与 tauri-specta 相同的 `events.xxx.listen(cb)` 形状。
 * 原版返回 Promise<UnlistenFn>，调用方大多写 `await events.x.listen(...)`，
 * 这里返回函数本身，await 一个函数同样是安全的。
 */
export const events = {
  downloadShelfEvent: {
    listen: (cb: (ev: { payload: DownloadShelfEvent }) => void) =>
      subscribe("downloadShelfEvent", cb),
  },
  downloadSleepingEvent: {
    listen: (cb: (ev: { payload: DownloadSleepingEvent }) => void) =>
      subscribe("downloadSleepingEvent", cb),
  },
  downloadSpeedEvent: {
    listen: (cb: (ev: { payload: DownloadSpeedEvent }) => void) =>
      subscribe("downloadSpeedEvent", cb),
  },
  downloadTaskEvent: {
    listen: (cb: (ev: { payload: DownloadTaskEvent }) => void) =>
      subscribe("downloadTaskEvent", cb),
  },
  downloadTaskDeletedEvent: {
    listen: (cb: (ev: { payload: DownloadTaskDeletedEvent }) => void) =>
      subscribe("downloadTaskDeletedEvent", cb),
  },
  exportCbzEvent: {
    listen: (cb: (ev: { payload: ExportCbzEvent }) => void) => subscribe("exportCbzEvent", cb),
  },
  exportPdfEvent: {
    listen: (cb: (ev: { payload: ExportPdfEvent }) => void) => subscribe("exportPdfEvent", cb),
  },
  logEvent: {
    listen: (cb: (ev: { payload: LogEvent }) => void) => subscribe("logEvent", cb),
  },
  /** WebSocket 建连时后端推的全量任务快照。 */
  taskSnapshot: {
    listen: (cb: (ev: { payload: DownloadTaskEvent[] }) => void) =>
      subscribe("taskSnapshot", cb),
  },
};

/** 供 UI 显示版本号用的静态值（原 `getVersion()` 的替代）。 */
export const APP_VERSION = "0.1.0";
