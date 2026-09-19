# wnacg-downloader Web 版 —— Docker 部署

把原 Tauri 桌面版改造成的 Web 版：Rust axum 服务端 + Vue3 前端，一个镜像跑起来。

## 快速开始

```bash
cp .env.example .env
# 按需改端口 / 代理
docker compose up -d --build
```

然后打开 `http://<NAS 的 IP>:8082`。

**不需要登录。** 本部署是局域网自用，`.env` 里已设
`WNACG_AUTH_DISABLED=true`，打开即用。

> ⚠️ 关闭认证后 `/api/config` 会**明文返回 wnacg 账号 Cookie**，
> 局域网内任何人 curl 一下就能拿到。请勿把本服务暴露到公网
> （内网穿透、端口转发、反代到公网都不行）。真要暴露就先把
> `WNACG_AUTH_DISABLED` 改回 `false`，或用反代自己加鉴权。

### 运行期代理（本机实测必须配）

NAS 直连 wnacg 各域名**全部超时**，不配代理会搜索转圈、下载全失败：

| 目标 | 直连 | 走 mihomo (7890) |
|------|------|------------------|
| www.wnacg.com | 000 超时 | 403（正常） |
| www.wn07.ru | 000 超时 | 301（正常） |

`.env` 里已配好，用 `host.docker.internal` 指向宿主机，换 NAS 不用改：

```
WNACG_HTTP_PROXY=http://host.docker.internal:7890
WNACG_HTTPS_PROXY=http://host.docker.internal:7890
```

验证容器内是否生效：

```bash
docker exec wnacg-downloader-web sh -c \
  'curl -s -o /dev/null -w "%{http_code}\n" https://www.wnacg.com/'
# 403 或 301 = 通了；000 = 没通
```

### 如果要重新启用认证

把 `.env` 里 `WNACG_AUTH_DISABLED` 改成 `false`，重建容器后从日志取令牌：

```bash
docker compose logs wnacg-downloader-web | grep 访问令牌
```

想固定令牌就写进 `.env` 的 `WNACG_AUTH_TOKEN`，重建也不会变。

## 端口分配

本机已有两个同族服务，端口约定如下（不要冲突）：

| 服务 | 宿主端口 |
|------|---------|
| picacomic-downloader-web | 8080 |
| jmcomic-downloader-web | 8081 |
| **wnacg-downloader-web** | **8082** |

改端口只需改 `.env` 里的 `WNACG_HOST_PORT`。

## 目录挂载

| 容器内 | 宿主默认 | 内容 |
|--------|---------|------|
| `/data` | `./data` | `config.json`、日志、`wnacg_server.db` |
| `/comic-download` | `./comic-download` | 漫画下载产物 |

⚠️ 漫画默认下载到 `/data/漫画下载`。想让产物出现在 `/comic-download`
挂载点里，**首次启动后要在网页「配置」里把 `downloadDir` 改成
`/comic-download`**。这是沿用桌面版的默认值，不是 bug。

两个目录都会在容器启动时自动创建，全新部署不会因为目录不存在而报错。

## 代理配置

两类代理是完全独立的，别搞混：

### 构建期代理（`BUILD_HTTP_PROXY` / `BUILD_HTTPS_PROXY`）

只作用于 `docker compose build`。国内直连 npm registry / crates.io /
github 通常不通，不配代理会卡在拉依赖。

**必须写宿主机的局域网 IP，不能写 `127.0.0.1`**（构建容器里的
127.0.0.1 指向它自己）：

```
BUILD_HTTP_PROXY=http://192.168.1.100:7890
BUILD_HTTPS_PROXY=http://192.168.1.100:7890
```

### 运行期代理（`WNACG_HTTP_PROXY` / `WNACG_HTTPS_PROXY`）

只作用于服务运行时访问 wnacg 站点和图片 CDN。

**现象**：网页能打开、能搜索、能登录，但点下载一直转圈或全部超时
—— 就是缺这个。

先在 NAS 上自测直连是否通：

```bash
curl -sS -o /dev/null -w '%{http_code}\n' https://www.wnacg.com/
```

- 超时 / 无输出 → 直连不通，必须配代理
- `200` / `403` → 通了，可以留空

配的时候推荐用 `host.docker.internal`（compose 里已通过
`extra_hosts` 映射到宿主机），换 NAS 或改 IP 时不用动配置：

```
WNACG_HTTP_PROXY=http://host.docker.internal:7890
WNACG_HTTPS_PROXY=http://host.docker.internal:7890
```

## 环境变量

| 变量 | 默认 | 说明 |
|------|------|------|
| `WNACG_HOST_PORT` | `8082` | 宿主端口 |
| `WNACG_AUTH_DISABLED` | **本部署为 `true`** | 关闭认证，打开即用。暴露公网必须改 `false` |
| `WNACG_AUTH_TOKEN` | 随机生成 | 仅在 `DISABLED=false` 时有意义；留空则每次启动重新生成 |
| `WNACG_AUTH_USERNAME` | `admin` | Basic 认证用户名（仅在启用认证时） |
| `WNACG_DATA_PATH` | `./data` | 数据目录 |
| `WNACG_COMIC_PATH` | `./comic-download` | 漫画目录 |
| `TZ` | `Asia/Shanghai` | 时区 |
| `WNACG_HTTP_PROXY` / `WNACG_HTTPS_PROXY` | **本部署已配 7890** | 运行期代理，不配则下载全超时 |
| `JM_LOG_LEVEL` | `INFO` | 日志级别。**默认 INFO 是刻意的**，TRACE 下每张图都打日志，曾撑爆磁盘 |

容器内固定使用 `WNACG_BIND=0.0.0.0`、`WNACG_PORT=8080`，一般不用改。

## 健康检查与常用命令

```bash
# 容器状态（healthy 才算正常）
docker ps --filter name=wnacg

# 健康检查端点
curl -s http://127.0.0.1:8082/api/health      # {"status":"ok"}

# 实时日志
docker compose logs -f wnacg-downloader-web

# 重启 / 停止
docker compose restart
docker compose down

# 彻底重建（改了 Rust 代码后必须加 --build）
docker compose up -d --build
```

## 架构

```
Dockerfile（3 阶段）
  ├─ web-builder    node + pnpm → vite build → /build/dist
  ├─ server-builder rust:bookworm → cargo build --release（LTO）→ wnacg-server
  └─ runtime        debian:bookworm-slim，非 root 用户 wnacg(uid 1000)
```

服务端单进程同时提供：

- 静态前端（`/app/dist`，SPA 回退到 `index.html`）
- REST API（`/api/*`）
- WebSocket（`/api/ws`，推送下载进度、日志等实时事件）

**认证默认关闭**（`WNACG_AUTH_DISABLED=true`），所有端点匿名可访问。
启用认证后，除 `/api/health` 和 `/api/auth/check` 外都需带令牌；
浏览器 WebSocket 发不了自定义请求头，那种情况下 `/api/ws` 支持
`?token=xxx` 查询参数认证。

## 排障

**网页打不开**
```bash
docker ps --filter name=wnacg          # 看是否 Up (healthy)
docker compose logs --tail 50
```

**能打开但要我登录**（本部署不该出现）
本部署认证是关的。若仍弹登录，检查 `.env` 里
`WNACG_AUTH_DISABLED=true` 是否生效（改完必须 `docker compose up -d` 重建容器）：
```bash
docker compose exec wnacg-downloader-web env | grep AUTH
```
若你确实启用了认证而令牌丢了，从日志重新取：
```bash
docker compose logs wnacg-downloader-web | grep 访问令牌
```

**「已下载漫画」页面报读取目录失败**
容器启动时会自动创建下载目录。若仍报错，检查 `/data` 卷是否可写：
```bash
docker compose exec wnacg-downloader-web ls -la /data
```

**下载全部超时**
没配运行期代理，见上文「运行期代理」。

**改了 Rust 代码但没生效**
`docker compose build` 有缓存。Dockerfile 里已对源码做 `touch`
规避 mtime 缓存问题，但改了 `Cargo.toml` 依赖仍需完整重建：
```bash
docker compose build --no-cache server
```
