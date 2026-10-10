# P0：文本契约与离线夹具（首个切片）

关联：[#1](https://github.com/hyperlook/deckcraft/issues/1)、[P0 #3](https://github.com/hyperlook/deckcraft/issues/3)。设计依据：[AI 助手 RFC](rfcs/ai-assistant.md)。

**这是部分 P0，不是 AI 用户功能。** 没有注册 `ai.applyPatch`，没有模型/网络/运行时或 UI，没有修改 `DocState`、历史或选区。P0 的模板追加草稿契约与 2–3 套样本选择仍待完成；P1 之前不能把这些类型当作可提交事务。

## 模块边界

```text
宿主：真实文档 + 用户授权
  │ 冻结 TextScope 与 TextRequest（仅必要文字）
  ▼
crates/ai (L6)：意图、任务状态；测试用 fake provider
  │ 原始响应 → 有界解析 → 受限 TextPatch
  ▼
crates/engine::ai_contract (L5)：快照、白名单、只读校验
  │ 本切片到此结束
  ▼
P1：检查真实文档身份/内容版本 → 候选副本 → 整体安装与一条历史
```

引擎不依赖 AI。AI 不拥有或锁定真实 `Session`，不引入异步运行时、文件读取、HTTP 或凭据。后续 UI/CLI 宿主适配任务控制与引擎提交；不把 provider 的响应解释成通用引擎命令。AI 是 L6 纯业务 crate，尚未建立 UI 到 AI 的依赖。

## 文本快照、请求与响应

- `SnapshotIdentity`：`job_id`、`doc_uid`、`base_content_revision`、稳定 `target_slide_ids`、`plan_version`、可选 `template_version`。文本切片要求非空目标；生成任务对空目标文档的语义将在模板草稿契约中补齐。
- `TextScope`：宿主冻结的可改 `(SlideId, ShapeId)` 白名单。选中一页不等于授权所有形状；不能从模型响应中重建授权。
- `TextRequest`：白名单、指令、受众、目标、叙事摘要、已确认事实/来源、禁改项，以及每个字段的原始段落和页面职责。不发送整个 Presentation、媒体、文件路径、凭据或聊天记录。来源只是用户给出的标签，不授予联网权限。
- `TextPatch`：完整快照身份与 `replace_text` 操作列表。字符串是纯文本，Markdown 标记不会变成富文本。空操作列表合法，供 P1 无变化提交语义使用。
- 请求正文必须与传入的冻结文档切片一致。响应快照必须完全匹配宿主的冻结授权；每个目标最多出现一次。形状 ID 是页内地址，同一个 ID 可以出现在不同页面。

当前只支持**可见、未锁定、顶层普通形状**的已有文本。每个段落内部 run 属性必须相同；段落之间可以格式不同，等格式的多个 run 可合并替换。不支持组内对象、表格、连接器、母版、备注、字段、数学内容或 run/段落结束标记上的链接；这些场景明确拒绝。

响应显式携带段落数组，段落数必须与原对象相同；字符串中拒绝控制字符（包括换行、软换行与 Tab），空字符串可以清空相应段落。后续执行器必须保留各段落的格式与对象结构，**不能通过现有有损 `text.set` 实现**。

Schema 每层拒绝未知字段，未知操作拒绝；无效/零 ID、重复或不明目标、越权目标与身份变化均返回类型化错误。解析错误不回显原始正文。确认事实是请求约束，不是已实现的事实正确性验证；这需要 P3 的验证与差异预览。

### 固定边界（可在后续版本显式调整）

| 项目 | 上限 |
|---|---:|
| JSON 响应/请求字节 | 256 KiB |
| 目标页面数 | 64 |
| 授权字段 / 补丁操作数 | 各 256 |
| 每对象段落数 / 每段原始 run 数 | 各 128 |
| 单对象替换文字 | 16,384 Unicode 标量值（不是字节或字形簇） |
| 全部响应文字 / 全部请求正文 | 各 128 KiB UTF-8 |
| 单个意图字段 / 事实 / 来源 / 页面职责 | 4,096 Unicode 标量值 |
| 确认事实 / 禁改项 | 各 64 |
| 任务 key | 1–64 字节 ASCII 字母、数字、`-`、`_` |
| fake 脚本响应数 | 64 |

`decode_bounded` 在 serde 分配前检查字节数，保留 serde 递归深度限制；解析后以及进程内直接构造的类型均需语义校验。宿主还必须在接收网络正文时限制流式读取大小，不能先无限缓冲再调用解码器。已有文档源字段也有 run 数和文字字节上限。

本阶段只验证传入的身份与快照，不从 `DocState::revision` 生成内容身份。P1 必须维护独立内容版本，提交时比较真实文档 UID/内容版本，并再次执行校验；选区变化不能造成冲突，真实修改和 Undo/Redo 必须造成冲突。

## 任务状态与事件权限

```text
planning → awaiting_outline → awaiting_template → generating → validating → ready → applying → applied
                                                      ↑            │          │         │
                                                      └─ retry ────┘          └─────────┴→ conflicted
```

局部文本任务从 `generating` 开始。`failed`、`cancelled`、`conflicted`、`applied` 是终态，不允许恢复到运行中。确认新大纲/模板/共同事实或终态后重新生成需创建新任务和身份。生成入口的状态路径仅为契约测试，并不表示大纲生成已经实现。

`TaskEvent` 是**宿主控制协议**，不是 provider 输出协议。只有宿主能发送生成结束、验证通过、应用请求与应用成功等事件；`ValidationPassed` 表示全部目标通过确定性验证，`Applied` 必须在引擎成功提交之后发送。无效状态跳转或身份不匹配不改变任务。终态收到匹配身份的晚到事件会丢弃；不匹配事件返回身份错误。

取消与应用在宿主线程串行处理：提交完成后必须先发 `Applied`，再处理排队的取消。此时取消无效，只能通过普通 Undo 撤销。文档关闭等同取消；冲突在 ready/applying 保留解释与草稿，失败只存枚举原因，不存敏感正文。

本切片没有逐页结果存储或请求池，因此还不证明真实异步响应的取消/乱序处理。P2 需实现并验证这些规则，以及并发、重试、费用、期限、RPM/TPM 和同文档单活任务。

## 离线复现与剩余工作

`crates/ai/tests/fixtures/` 包含三页小文档、两页授权请求与固定响应。其中未授权字段带有标记，测试验证它们不会被上传。`fake` 模块仅在单元测试或 `test-support` feature 下可用；原始脚本响应走真实解码和边界校验，支持失败与耗尽，不持久化 prompt 或正文。

```bash
cargo test --offline -p deckcraft-ai --features test-support
cargo test --offline -p deckcraft-engine
cargo test --offline -p xtask
cargo fmt --all -- --check
cargo clippy --offline --all-targets --no-deps -p deckcraft-engine -p deckcraft-ai --features deckcraft-ai/test-support -- -D warnings
cargo xtask layers
```

没有可截图的 UI 或可渲染的修改结果，视觉验收不适用于这次只读契约切片。`cargo xtask wasm` 已把 AI 纯业务 crate 纳入检查集合，但实际 WASM 编译、工作区 CI 与模板视觉/PPTX 回读仍需按阶段验收，不以局部测试替代。

P0 剩余：模板追加草稿的完整资源/引用及局部 ID 契约；2–3 套样本的页型、字体与资源记录。不得把未确认许可的本地模板库直接提交到仓库，也不将裸页面数组暂称为安全模板补丁。

**关键决策**：将授权和提交白名单留在引擎边界，避免 provider 扩权；使用显式段落替换并保持数量，避免首版文本精简先损坏格式，再由历史/预览补救。
