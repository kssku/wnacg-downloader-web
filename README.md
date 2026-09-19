<p align="center">
    <img src="https://github.com/user-attachments/assets/0e266cd6-10db-4470-96ce-68d548363ae4" style="align-self: center"/>
</p>

# 📚 绅士漫画下载器（Web 版）

一个用于 wnacg.com 绅士漫画 的多线程下载器，带收藏夹，下载速度飞快。

本仓库是 [wnacg-downloader](https://github.com/lanyeeee/wnacg-downloader) 的 **Web 化改造版**：
把原来的 Tauri 桌面壳换成 **Rust axum 服务端 + Vue3 前端**，用 Docker 部署，
浏览器打开就能用 —— 不需要在每台设备上装桌面程序。

改造范式与这两个项目保持一致：

- [jmcomic-downloader-web](https://github.com/lanyeeee/jmcomic-downloader-web)
- [picacomic-downloader-web](https://github.com/lanyeeee/picacomic-downloader-web)

# ✨ 相比桌面版的变化

| | 桌面版（原版） | 本仓库（Web 版） |
|---|---|---|
| 界面 | Tauri 桌面窗口 | 浏览器页面 |
| 后端 | `src-tauri`（内嵌 Tauri 运行时） | `src-server`（纯 axum，无 Tauri 依赖） |
| 部署 | 每个设备装一份安装包 | 一台机器跑 Docker，全屋设备浏览器访问 |
| 打开目录 | 调系统文件管理器 | 直接在服务端目录里找（挂载卷） |
| 事件推送 | Tauri event | WebSocket |
| 认证 | 无 | 访问令牌（Bearer / Basic / `?token=`） |

功能面保持一致：漫画搜索、标签搜索、书架、一键下载、暂停/继续/取消、
已下载漫画管理、导出 PDF / CBZ、日志面板、配置项（代理 / 并发 / 目录等）。

# 🚀 快速开始（Docker）

```bash
git clone <本仓库地址>
cd wnacg-downloader
cp .env.example .env
# 按需修改端口、代理、存储路径
docker compose up -d --build
```

然后浏览器打开 `http://<部署机 IP>:8082`。

**首次登录**：容器第一次启动会随机生成访问令牌，并只打印一次：

```bash
docker compose logs wnacg-downloader-web | grep 访问令牌
```

把令牌填进网页的登录框即可。

详细的部署说明（端口冲突、代理配置、目录挂载、常见问题）
见 **[DOCKER.md](./DOCKER.md)**。

# 🛠️ 本地开发

#### 📋 前提

- [Rust](https://www.rust-lang.org/tools/install)
- [Node](https://nodejs.org/en)
- [pnpm](https://pnpm.io/installation)

#### 📝 步骤

```bash
pnpm install

# 终端 1：起后端（默认监听 8080，数据目录 ./data）
cd src-server && cargo run

# 终端 2：起前端（Vite dev server，自动代理 /api 到后端）
pnpm dev
```

#### 构建产物

```bash
# 只构建前端静态资源到 dist/
pnpm build

# 只构建服务端二进制
cd src-server && cargo build --release
```

# 📖 使用方法

#### 🚀 不使用书架

1. **不需要登录 wnacg**，直接使用 `漫画搜索`
2. 直接点击卡片上的 `一键下载`，或者点封面 / 标题进入 `漫画详情`，里面也有 `一键下载`
3. 下载完成后到 `本地库存` 查看，也可以在部署机的下载目录里直接翻

#### ⭐ 使用书架

1. 点击 `账号登录` 按钮完成 wnacg 账号登录
2. 使用 `我的书架`，直接点击卡片上的 `一键下载`
3. 下载完成后到 `本地库存` 查看

**顺带一提，你可以在 `本地库存` 导出为 pdf / cbz(zip)**

# 🤝 提交 PR

**如果想新加一个功能，请先开个 `issue` 或 `discussion` 讨论一下，避免无效工作**

其他情况的 PR 欢迎直接提交，比如：

1. 🔧 对原有功能的改进
2. 🐛 修复 BUG
3. ⚡ 使用更轻量的库实现原有功能
4. 📝 修订文档
5. ⬆️ 升级、更新依赖的 PR 也会被接受

# ⚠️ 免责声明

- 本工具仅作学习、研究、交流使用，使用本工具的用户应自行承担风险
- 作者不对使用本工具导致的任何损失、法律纠纷或其他后果负责
- 作者不对用户使用本工具的行为负责，包括但不限于用户违反法律或任何第三方权益的行为

# 💬 其他

任何使用中遇到的问题、任何希望添加的功能，都欢迎提交 issue 或开 discussion 交流。