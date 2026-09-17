# Ternion

一個介面。三種模型。每則訊息都用最合適的模型。

> [English](./README.md) | **繁體中文**

**Ternion** 是一款本機優先的 Windows 桌面應用程式，用來與大型語言模型
（LLM）對話與協作。它原生支援 Ollama，也支援任何 OpenAI 相容端點 —
而且不用你為每則訊息挑模型：它的 **Triad 路由系統**會替你決定 — 小事
用小而快的模型，難事用大而強的模型，由一個輕量分類器逐訊息自動路由。

## Triad 系統

Ternion 是 **Triad 模型路由系統**（Herald / Scout / Titan）的參考實作：

- **Herald** — 輕量分類器，讀取每則訊息並決定由誰回答（一次結構化輸出
  呼叫，溫度 0、信心門檻控管）
- **Scout** — 小而快的工作者：招呼、快速查詢、短文改寫、單檔編輯
- **Titan** — 大而強的工作者：多步驟推理、架構層級的程式工作、長篇
  文件、多工具代理任務

路由優先序：硬性規則 → Herald → 啟發式 → 預設 Scout。每次決策都會
保存且可檢視。Herald 沒把握時由信心門檻落到啟發式；硬性規則始終修正
能力缺口（例如：附圖即強制切換到具視覺能力的模型）。

身為使用者你會看到：每則訊息的訊息列標示「由誰回答、為什麼」；釘選
晶片（自動 / Scout / Titan / 指定模型）；每個對話的**路由紀錄**；以及
全域彙總的 **Triad 報表**。模型切換走滾動摘要交接，新模型能接續任務；
Scout 超出輸出上限時提供一鍵「以 Titan 接續」。

## 亮點

- **串流對話** — 逐 token 輸出、thinking 模型的思考區塊、真正釋放
  Ollama 運算槽的停止鍵，以及每則訊息的使用量（輸入/輸出 token、延遲）
- **完整對話歷史** 保存於 SQLite（WAL）— 建立 / 重新命名 / 刪除 /
  搜尋、自動命名
- **檔案系統代理能力** — 每個對話綁定至多 3 個工作區資料夾；12 個內建
  `fs_*` 工具背後有 Windows 路徑防護（canonicalize + 不分大小寫的前綴
  檢查）；修改型工具走授權矩陣（詢問 / 本次工作階段允許 /
  此資料夾一律允許）並附統一 diff；選用開啟的 `shell` 工具，每道命令
  都需授權
- **可擴充工具** — 本機 stdio MCP 伺服器與 **OpenWebUI Skills**（Python
  `class Tools` 資訊清單，相容於 OpenWebUI 由 Tools 更名為 Skills 的
  格式），併入同一個以命名空間分隔、經權限閘門的工具介面
  （`mcp__<伺服器>__<工具>`、`owui__<清單>__<方法>`）。OpenWebUI
  **Functions**（Filters / Pipes）尚未支援 — 見路線圖。
- **上下文壓縮** — 當路由模型的視窗將滿，較舊歷史會漸進式摘要成一份
  持續併入的儲存摘要；近期結尾保持原文；失敗時降級為單純截斷，絕不
  阻塞
- **視覺** — 貼上、拖放、挑選或擷圖（Win+Alt+S）；EXIF 修正、長邊
  ≤ 1568 縮小、JPEG q85；附圖時視覺感知路由自動升級模型
- **多端點** — 具名 Ollama / OpenAI 相容設定檔，API 金鑰存於 Windows
  認證管理員；每組（端點，模型）的能力登錄；跨提供者模型參照
  （`model@endpoint_id`）
- **提供者交接提示** — 以計時器探測端點；某端點上線時提供切換詢問
  （立即切換 · 保留目前 · 不再詢問），雲端附費用註記、僅本機模式下
  隱藏
- **快速擷取調色盤** — Win+Alt+T 開啟無邊框、置頂的快速提問列，走
  完整 Triad 管線並存成真正的對話
- **僅本機模式** — 一個開關（設定或系統匣）停用所有雲端端點；路由退回
  最佳本機模型，當沒有本機模型可服務時明確失敗而非悄悄外洩
- **淺色 / 深色 / 跟隨系統佈景主題**、不閃白開機，以及完整的
  繁體中文（zh-TW）介面
- **Windows 原生封裝** — NSIS 個人層級安裝程式（English/繁體中文）、
  單一執行個體、關閉時縮到系統匣且串流持續，以及對準 GitHub Releases
  的休眠式 tauri 更新機制

## 安裝

預先建置的安裝程式附於
[GitHub Releases](https://github.com/SynesisLab/Ternion/releases/latest)
（`Ternion_0.1.0_x64-setup.exe`，個人層級安裝、English/繁體中文）。
目前 binaries 未簽署 — SmartScreen 會詢問；請選擇
*更多資訊 → 仍要執行*。

也可以自行建置：

```sh
npm install
npm run tauri build
```

安裝程式位於 `src-tauri/target/release/bundle/nsis/`，獨立執行檔位於
`src-tauri/target/release/`。

環境需求：Windows 10 21H2+ / Windows 11、[Node.js](https://nodejs.org)
20+（npm）、Rust `stable-msvc` 工具鏈 + Visual Studio 2022 Build Tools
（含 C++ 桌面開發工作負載）、本機執行的
[Ollama](https://ollama.com)（預設 `http://127.0.0.1:11434`），以及
WebView2 執行階段（Windows 11 已內建）。開發模式：

```sh
npm install
npm run tauri dev
```

## 快速鍵

| 組合鍵       | 動作                                       |
| ------------ | ------------------------------------------ |
| `Win+Alt+S`  | 區域擷圖 → 附件落入輸入區                  |
| `Win+Alt+T`  | 快速提問調色盤（螢幕右上）                 |

兩者皆全域註冊：若其他應用程式已佔用某組合，Ternion 會在開機時記錄
警告並在缺少該快速鍵的狀態下啟動 — 不影響應用程式本身，僅該快速鍵
失效。

## 資料位置

一切本機優先，都在 `%LOCALAPPDATA%\com.paperplane.ternion\`：

- `ternion.db` — 對話、訊息、路由事件、工具呼叫（SQLite，WAL）
- `attachments\` — 處理後的圖片檔（資料庫列只存路徑）
- `logs\ternion.log` — 應用程式與 webview 診斷

除非你主動指定，資料不會離開這台機器：未設定雲端端點、且系統匣有
僅本機模式時，Ternion 只與你的本機 Ollama 溝通。模型圖片以 sha256
為鍵儲存在本機；API 金鑰存於 Windows 認證管理員，絕不寫入資料庫。

## 測試

```sh
cd src-tauri
cargo test            # 單元測試 — 不需要 Ollama
cargo test -- --ignored   # 針對執行中 Ollama 的即時探測
```

前端經 `npm run build` 進行型別檢查。

## 給貢獻者的架構筆記

`src-tauri/src/chat.rs` 負責單一串流編排（防護、佔位列、節流保存、
取消）；`src-tauri/src/providers/` 將每個提供者編譯為 `types.rs` /
`src/types/stream.ts` 的標準化串流通訊協定（有一個單元測試守護通訊
協定的一致性）；`src-tauri/src/router/` 是 Triad 路由器 — Herald
分類、啟發式政策引擎、交接摘要與上下文壓縮。前端在每次串流結束後自
資料庫重新取得資料 — 資料庫是唯一事實來源。完整產品設計見
[DESIGN.md](./DESIGN.md)。

## 路線圖

目前刻意不做：串流中的提供者失效重試、mDNS 端點探索、OpenWebUI
Functions（Filters / Pipes）作為中介層、以及安裝程式的程式碼簽署。

## 發佈與更新

在實際的發佈管線存在之前，tauri 更新機制已就緒但維持休眠。啟用
步驟：

1. 產生簽署金鑰對（私密金鑰不要放進儲存庫）：
   ```sh
   npm run tauri signer generate -w ./ternion.key
   ```
2. 將公開金鑰填入 `src-tauri/tauri.conf.json` 的
   `plugins.updater.pubkey`（取代預留值）。
3. 以私密金鑰作為環境變數建置（`.env` 檔對此無效）：
   ```sh
   TAURI_SIGNING_PRIVATE_KEY=$(cat ternion.key) npm run tauri build
   ```
   這會在安裝程式旁產生已簽署的 `.nsis.zip` 更新成品。
4. 發佈：將安裝程式、已簽署的更新成品與產生的 `latest.json` 清單附加
   到 GitHub Release。

## 授權

[MIT](./LICENSE) — © 2026 SynesisLab