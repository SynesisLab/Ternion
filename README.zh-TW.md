# Ternion

一個介面。三種模型。每則訊息都用最合適的模型。

> **Ternion 是 Triad 模型路由系統的概念驗證** — Herald / Scout / Titan
> 架構：由一個輕量分類器將每則訊息路由到最合適的模型（自 M3 起也路由到
> 最合適的端點）。

[English](./README.md) | **繁體中文**

Ternion 是一款本機優先的 Windows 桌面應用程式，用來與大型語言模型（LLM）
對話與協作。它原生支援 Ollama，也支援任何 OpenAI 相容端點。完整產品設計
（Triad 系統：Herald / Scout / Titan 路由）請見
[DESIGN.md](./DESIGN.md)。

## 狀態 — M0–M4 完成（Triad 路由器、工具、視覺、打磨）

**基礎（M0）**

- **串流對話**：支援任何 Ollama 模型 — 逐 token 顯示、思考區塊（thinking
  模型）、中途停止（釋放 Ollama 運算槽）、每則訊息的使用量
  （輸入/輸出 token、延遲）
- **對話**：建立 / 重新命名 / 刪除 / 搜尋；以第一則訊息自動命名；完整
  歷史保存於 SQLite（WAL）
- **Markdown**：GFM 表格、語法高亮的程式碼、連結以系統瀏覽器開啟
  （模型輸出的 HTML 一律跳脫 — 不渲染原始 HTML）
- **模型**：從 `/api/tags` 即時探索，附能力徽章（視覺 / 工具 / 思考）
- **設定**：Ollama 基礎網址（含連線測試）、生成預設值（溫度、上下文
  tokens、keep-alive）、關閉時縮到系統匣
- **系統匣**：顯示/隱藏 · 新對話 · 結束；縮到系統匣時串流持續運行；
  單一執行個體（再次啟動會聚焦現有視窗）

**Triad 路由器（M1）** — DESIGN.md §3 的 Herald / Scout / Titan 系統：

- **路由優先序**：硬性規則 → Herald（結構化分類呼叫，信心 ≥ 0.65、溫度
  0）→ 啟發式 → 預設 Scout；每次決策都寫入 `routing_events` 資料表，並
  關聯回訊息歷史
- **釘選**：自動 / Scout / Titan 晶片，或釘選指定模型；各角色模型指派於
  設定 → Triad
- **交接**：回合之間切換模型時，新模型會收到較早任務狀態的滾動摘要，加
  上路由器的交接備註；訊息列會標示切換（⚡）
- **黏滯**：Scout 輸出超過上限時提供「以 Titan 接續」；Titan 會黏著
  `sticky_turns` 個回合才降級
- **附屬任務**（僅 Herald、發後即忘）：對話標題精修、摘要維護、後續
  建議晶片
- **路由紀錄**：每個對話的抽屜面板，顯示每次決策的來源、信心、估計
  tokens 與覆寫種類

**工具與檔案系統代理能力（M2）** — DESIGN.md §6：

- **工作區**：每個對話最多綁定 3 個資料夾（表頭列）；未綁定工作區時模型
  對檔案零存取權 — `tools` 陣列甚至不會宣告
- **Windows 路徑防護（§6.3）**：模型提供的每個路徑都經 canonicalize 與
  不分大小寫的前綴檢查 — 不允許 verbatim 前綴磁碟機、`..` 逃逸、裝置
  名稱（`CON`、`NUL`）、junction 逃逸；建立路徑會探測最深層的已存在
  上層目錄
- **讀取工具（自動允許）**：`fs_list`、`fs_read`（編號行、offset/limit）、
  `fs_stat`、`fs_search`（內建 ripgrep 引擎 — 正規表示式、glob 過濾、
  遵循 `.gitignore`）、`fs_tree`；每個結果都有明確的截斷註記
- **修改型工具（權限矩陣 §6.6）**：`fs_write`（原子性 tmp+rename、附加
  模式）、`fs_edit`（精確字串取代）、`fs_move`、`fs_copy`、`fs_delete`
  （僅移至資源回收筒 — 永不永久刪除）、`fs_mkdir`
- **授權**：依（工具 × 工作區）可選 詢問 / 本次工作階段允許 /
  此資料夾一律允許；`fs_write`/`fs_edit` 的授權對話框附統一 diff 與完整
  結果內容，包括就地編輯（您修改後的版本會成為工具自己的參數）；請求
  待決時串流暫停、絕不取消；按下停止會拒絕待決呼叫。於
  設定 → 權限 管理
- **Shell 工具（選用開啟）**：`shell` 以 PowerShell 執行（7 → 5.1 後備），
  工作目錄固定在工作區內；輸出上限 8 KB、逾時即終止，且每道命令都需
  授權 — shell 命令永不列入白名單
- **工具迴圈**：驗證 → 權限閘門 → 執行 → `<tool_result
  source="untrusted">` 回傳給模型，至多 `tools.max_hops`（12）次後強制
  以無工具模式總結；每次呼叫連同結果與權限模式一併保存；工具活動即時
  顯示於對話中

**視覺與多端點（M3）** — DESIGN.md §5、§7：

- **端點設定檔**：具名 Ollama / OpenAI 相容端點（設定 → 端點），含基礎
  網址、選用的 API 金鑰、自訂標頭、啟用開關，以及會列出探索到模型的
  連線測試
- **OpenAI 相容轉接器**：SSE 串流（含工具呼叫）編譯為與 Ollama 轉接器
  相同的標準化串流通訊協定
- **模型參照**：裸名稱代表內建 Ollama；`model@endpoint_id` 代表設定檔 —
  Triad 角色、釘選與能力紀錄皆按端點解析，對話中途的升級可將回合跨
  提供者移轉
- **能力登錄**：每組（端點，模型）的事實資料 — 能力、上下文長度、角色、
  VRAM 估計 — 透過 Ollama `/api/show` 驗證或手動編輯（設定 → 模型 分頁）
- **圖片附件（§7.2）**：貼上、拖放、檔案選擇器或下方的區域擷取 → 解碼 →
  EXIF 方向修正 → 縮小（長邊 ≤ 1568）→ JPEG q85；檔案以 sha256 為鍵
  儲存在資料庫旁，資料庫列只保存路徑，base64 內容於每次傳遞提供者時
  重新產生
- **視覺路由（§7.6）**：附加任何圖片都會強制使用具視覺能力的模型 —
  自動回合升級（Scout → 具視覺的 Scout → Titan → 第一個具能力的登錄
  模型）；已釘選回合則顯示警告晶片與一鍵修正，而不會被覆寫
- **區域擷取**：`Win+Alt+S` 在游標所在螢幕上開啟準星覆層；拖曳矩形
  （Esc 取消）經 IMG 管線處理後落入輸入區，成為待傳附件晶片

**打磨（M4）** — DESIGN.md §3.11、§9.3、§9.5、§14：

- **Triad 報表**（設定 → Triad）：跨所有路由事件與完成回合的全域彙總 —
  決策來源（Herald / 啟發式 / 硬性規則 / 手動）、升級與降級次數、覆寫、
  Herald 平均延遲、各角色回合數 / 延遲 / token 消耗，以及相較於一律用
  Titan 的估計節省時間（附其使用的觀測基準）
- **自適應調校**（選用）：與對話最新自動決策相抵的釘選，會微調該旗標
  類別的升級門檻 — 學到的調整（含覆寫次數）顯示於報表，且可重設
- **快速擷取（Win+Alt+T）**：位於螢幕右上、無邊框且置頂的調色盤視窗；
  提問走完整 Triad 管線並存成真正的對話 — 路由訊息列、圖片貼上、區域
  擷取，以及一鍵交接回主視窗
- **僅本機模式**（設定 → 一般，或系統匣）：一個隱私開關，停用所有雲端
  端點 — 雲端釘選/角色退回該角色可用的最佳本機模型，Herald 視為離線
  （改由啟發式決定），視覺升級絕不離開內建 Ollama；當沒有本機模型可以
  服務時，該回合會明確失敗而非悄悄外洩
- **繁體中文**：完整的 zh-TW 語系目錄，附語言設定（模型代號維持英文）
- **更新程式與封裝（§14）**：NSIS 個人層級安裝程式（English/繁體中文），
  以及對準 GitHub Releases 的 tauri 更新機制 — 在簽署金鑰對產生前維持
  休眠（見下方「封裝與更新（§14 M4）」一節）

**v1 後待辦（已全部完成）** — M4 之後仍留在待辦清單的 §6.5/§5.6 項目：

- **淺色佈景主題（§10）**：跟隨系統模式切換的淺色配色代幣，開機時先套
  用主題、不閃白
- **MCP 用戶端（§6.5）**：在設定中配置本機 stdio 伺服器；其工具以
  `mcp__<伺服器>__<工具>` 命名合併，並如 shell 一樣閘門管控 — 每次呼叫
  先詢問、可按工具授權，設定變更即清空工作階段快取
- **上下文壓縮（§6.5c）**：當組裝後的上下文逼近路由模型的視窗時，較舊的
  歷史會被漸進式摘要（指派了 Herald 就用 Herald，否則用被路由到的模型）
  成一份會持續併入的儲存摘要；近期結尾保持原文，失敗時降級為單純截斷
  並附註記 — 絕不阻塞
- **OpenWebUI 工具（§6.5b）**：OpenWebUI「Tools」資訊清單（Python
  `class Tools`）解析為 JSON-schema 規格，以 `owui__<清單>__<方法>` 命名
  合併，經本機 Python 直譯器透過產生的 shim 執行 — 與 MCP 工具相同的
  §6.6 閘門
- **提供者交接提示（§5.6）**：以計時器探測端點；當某個端點上線時，UI 提供
  切換詢問（立即切換 · 保留目前 · 此端點不再詢問，「一律」會自動套用）—
  僅本機模式會隱藏非本機的提議，雲端提議附費用註記，切換走 §3.6 摘要
  交接機制

## 環境需求

- Windows 10 21H2+ / Windows 11
- [Node.js](https://nodejs.org) 20+（npm）
- [Rust](https://rustup.rs) — `stable-msvc` 工具鏈 + Visual Studio 2022
  Build Tools（安裝「使用 C++ 的桌面開發」工作負載）
- [Ollama](https://ollama.com) 於本機執行（預設
  `http://127.0.0.1:11434`）
- WebView2 執行階段（Windows 11 已內建）

## 開發

```sh
npm install
npm run tauri dev
```

SQLite 資料庫位於 `%LOCALAPPDATA%\com.paperplane.ternion\ternion.db`，
附件檔在其旁的 `attachments\`；日誌在
`%LOCALAPPDATA%\com.paperplane.ternion\logs\ternion.log`。

## 建置

```sh
npm run tauri build
```

會產生 NSIS 安裝程式（個人層級安裝、English/繁體中文），位於
`src-tauri/target/release/bundle/nsis/`（未簽署 — SmartScreen 會提出
警告；程式碼簽署屬發佈階段決策，見下方）。

## 封裝與更新（§14 M4）

在實際的發佈管線存在之前，更新程式已就緒但處於休眠：更新從
`SynesisLab/Ternion` GitHub Releases 拉取（安裝程式資產旁的
`latest.json`），以 minisign 金鑰驗證，並在 Windows 上被動安裝。正式
發佈前的啟用步驟：

1. 產生簽署金鑰對（私密金鑰不要放進儲存庫 — 存放於密碼管理器／CI
   祕密）：
   ```sh
   npm run tauri signer generate -w ./ternion.key
   ```
2. 將產生的公開金鑰填入 `src-tauri/tauri.conf.json` 的
   `plugins.updater.pubkey`（取代預留值）。
3. 以私密金鑰作為環境變數進行建置（`.env` 檔對此無效）：
   ```sh
   TAURI_SIGNING_PRIVATE_KEY=$(cat ternion.key) npm run tauri build
   ```
   這會在安裝程式旁產生已簽署的 `.nsis.zip` 更新成品。
4. 發佈：將安裝程式、已簽署的更新成品與 `latest.json` 清單附加到 GitHub
   Release（當 `createUpdaterArtifacts` 開啟時，清單會自動寫出，含
   `-should-sign` 中繼資料）。

## 測試

```sh
cd src-tauri
cargo test            # 單元測試 — 不需要 Ollama
cargo test -- --ignored   # 針對執行中的 Ollama 的即時探測
```

給貢獻者的架構筆記：`src-tauri/src/chat.rs` 負責單一串流的編排
（防護、佔位列、節流保存、取消）；`src-tauri/src/providers/` 將每個
提供者編譯為 `types.rs` / `src/types/stream.ts` 的標準化串流通訊協定
（有一個單元測試守護通訊協定的一致性）；前端在每次串流結束後自資料庫
重新取得資料 — 資料庫是唯一事實來源。