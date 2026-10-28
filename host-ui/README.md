# Remote Host UI

Remote Host 的 Tauri 2 界面：菜单栏 / 托盘、设置、首次向导、新设备确认弹窗。网页部分在任何系统上都能开发和测试；`src-tauri/` 是 Tauri 外壳，负责窗口和与 `rs-host-agent` 的本地 IPC。

## 安装

要求 Node.js >= 22.11.0、pnpm 11.1.0。

```sh
cd host-ui
pnpm install
```

## 在浏览器中开发

```sh
pnpm dev
```

打开 <http://localhost:1420/#/dev>。不在 Tauri 中运行时，平台层和 IPC 自动使用假实现（`FakePlatform`）。可在 URL 上追加场景参数：

```text
http://localhost:1420/#/menu?scenario=controlled
```

可用场景：`controlled`、`idle`、`handoff`、`approval`、`update`、`update-ready`、`setup`、`offline`。

## 检查与测试

```sh
pnpm typecheck
pnpm lint
pnpm format:check
pnpm test
pnpm build
pnpm install --frozen-lockfile
```

## Tauri 应用

在 macOS / Windows 上运行完整应用：

```sh
pnpm tauri dev
```

没有真实 agent 时，先启动假 agent：

```sh
pnpm fake-agent --socket <路径>
```

## 目录

```text
src/
├─ app/        # 路由、页面登记、应用外壳、本地标志
├─ platform/   # Tauri / 假平台适配层
├─ ipc/        # 与 agent 的 JSON-RPC：类型、客户端、假实现
├─ theme/      # 设计 token 与全局样式
├─ i18n/       # 按页面拆分的文案
├─ pages/      # 每个窗口一个页面目录；设置分区在 settings/<id>/
└─ test/       # Vitest 初始化
```

页面样式一律放在同目录的 `*.module.css`，只使用 `src/theme/global.css` 中的 `--rs-*` 变量。
