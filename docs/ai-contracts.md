# P0：文本与模板追加契约、离线夹具

关联：[#1](https://github.com/hyperlook/deckcraft/issues/1)、[P0 #3](https://github.com/hyperlook/deckcraft/issues/3)。设计依据：[AI 助手 RFC](rfcs/ai-assistant.md)。

**这是只读 P0 契约，不是 AI 用户功能。** 本文件不安装补丁、不改历史。提交事务在 [P1 原子应用](ai-apply.md)。模板追加契约已有真实内存资源包与完整局部映射；3 套本地样本记录已在 `docs/ai-samples.md` 与 `samples-catalog.json` 规范化。

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

- `SnapshotIdentity`：`job_id`、`doc_uid`、`base_content_revision`、稳定 `target_slide_ids`、`plan_version`、可选 `template_version`。文本切片要求非空目标；`validate_for_append` 允许空目标集合，仅用于生成/追加。
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

本阶段只验证传入的身份与快照，不从 `DocState::revision` 生成内容身份。提交边界的独立内容版本、选区不冲突、真实修改和 Undo/Redo 冲突见 [P1](ai-apply.md)。

## 模板追加：provider 配方与宿主完整草稿是两层协议

`engine::ai_contract::template` 实现以下只读流水线：

```text
宿主通过 format API 加载独立本地工作副本（本模块无文件读取）
  → TemplateBundle::freeze(version, native Presentation)
  → AppendScope::freeze(snapshot, Arc<bundle>, page/slot 白名单, max_instances, target)
  → ai::template::TemplateRequest（仅授权语义文字/段落容量/页链接 key）
  → AppendResponse::decode（append_template 配方）
  → AppendDraft::bind（保留资源包 + 分配完整局部映射）
  → AppendDraft::validate（引擎独立重检）
  → P1 的候选副本与一次安装（本阶段没有安装）
```

### 所有权与授权

`TemplateBundle` **拥有实际 native 对象**，不是文件路径、catalog key 或以后才加载的句柄。
它保留不可变 `Arc<Presentation>`：页面、母版、内联主题、布局、默认文字样式、内嵌媒体字节
都可从 `native()` 在内存访问。`AppendScope`、`AppendDraft` 持有这个同一资源包的 Arc；
外部 COW 克隆或源文件变化不会改变冻结对象。包、scope、draft 不提供反序列化或写接口；
只有宿主调用冻结构造器授予权限，provider 不能用响应重建 scope。

宿主授权 `TemplatePage { key, source_slide_id, slots: [{ key, shape_id }] }`；key 与原生 ID
的映射只留在宿主。可改槽位复用文本契约的安全规则（顶层、可见、未锁定、段内等格式、无 run 链接）。
没列入白名单的原生对象不会上传。`TemplateRequest` 仅发送各授权页/槽位 key、原段落、文字容量、
链接页 key 与结构化意图，不发送 master/media/geometry/source ID/路径/整个模板或文档。
请求必须与冻结原文、槽位和容量完全一致，不能伪造或加入未授权字段。

追加身份必须指定匹配的非零模板版本；它的 `target_slide_ids` 冻结目标文档全部已有页的有序 ID，
空文档用 `[]`。非空目标还须匹配画布、默认文字样式与页脚设置，拒绝静默全局改样式。
空目标可以采用包的画布/默认样式/页脚设置，已有母版不能成为默认回退。
`TextScope`、`Task::text_edit` 仍拒绝空目标；`Task::outline_generation` 允许空目标。
这些检查不是实际 doc UID/content revision 校验；提交边界的比较见 [P1](ai-apply.md)。

### 唯一允许的 provider 响应

```json
{
  "op": "append_template",
  "snapshot": {
    "job_id": "draft-1", "doc_uid": 17, "base_content_revision": 4,
    "target_slide_ids": [], "plan_version": 1, "template_version": 9
  },
  "pages": [{
    "page_key": "points", "local_id": 1,
    "replacements": [{ "slot_key": "body", "paragraphs": ["新的结论"] }],
    "links": []
  }]
}
```

这是授权模板页的**有序实例配方**，不是裸页面数组、任意模型/几何序列化或引擎命令。
每实例至少有一个唯一非零 `LocalId`；只能追加，不能定位目标已有页去替换/删除。
段落数量保持不变，未指定的槽位保持源文本（P4 的样例残留检查须在 ready 之前执行）。
所有 wire 层拒绝未知字段；解码与进程内 typed response 都走相同语义校验。

### 资源闭包与局部 ID

冻结包从真实对象提取每页的完整复制闭包：该页、选中布局、真实母版（**仅保留该实例选中布局**）、
三层全部原生形状、主题/背景/文字/list style 中的媒体依赖。主题内联在母版，没有虚构 theme ID。
组形状按 owner 递归检查；shape ID 仅要求同一个 slide/master/layout owner 内唯一。
无效/重复/缺失 master/layout/media/shape/slide、悬挂页链接均拒绝；不使用 `master_for` 的首母版回退。

`AppendDraft::bind` 确定性地产生 `ResourceMapping { instance, source: SourceResource, local: LocalId }`。
每个复制实例得到自己的完整 master/layout/shape/media 映射，所有 local ID 全局唯一。
SourceResource 是带 kind/owner 的原生地址，LocalId 是草稿地址，两者都**不是目标文档 ID**；
映射不来自模型、不上传媒体。`validate()` 对映射缺失、额外、重复、零 ID、错误 instance、错误 slide
local 绑定及重复 local ID 均拒绝。local 分配是 checked arithmetic，耗尽返回错误，不饱和复用。

跨页链接不是资源递归遍历；它们是明确有界的图边，可以包含自链接/环。
每实例必须为它引用的每个 source page 给出一个
`PageLink { target_page_key, target_local_id }`，且这个 local 必须是本次配方中同一 source 页型的实例。
重复复制页可以明确指向不同副本；自链接也须指定副本。不指向旧样本页、目标已有页或“第一个同型页”，
不猜测链接目的地。固定 HTTPS URL 随宿主资源复制，provider 不能创建或改写 URL；程序、文件、
相对导航、自定义播放等 action 暂不支持，凭据/控制字符 URL 拒绝。

提交时重检身份、目标与 `draft.validate()`；在独立候选中为**所有** local resource
分配 fresh destination ID，按完整 mapping 复制/裁剪母版布局、主题、媒体与形状，再按实例的明确
PageLink 重写所有 shape/run/list style 页链接。所需资源已在内存，不得安装后临时读文件/网络补足。
空目标的全局设置须在候选内准备；非空目标不改旧页/全局样式。只在全部复制、资源和验证成功后整体安装。
P4 的模板加载、填槽、字体/布局/媒体解码、样例清理与渲染验证消费这些原生对象和确定性映射，
不能绕过 scope，或把“能 bind”当作“可提交/视觉正确”。

### 明确支持子集与固定上限

原生普通形状（含自定义 native path）、图片、组可作固定对象；媒体仅允许已内嵌 PNG/JPEG。
连接器、表格、图表、音视频、Ink、Opaque/raw XML、动画/过渡、非空备注、评论、
notes/handout master、section/custom show 和嵌入字体包暂不支持，明确拒绝而非有损复制。
图片头必须可读且宽高有界；这是资源契约检查，不是完整图片解码或渲染 QA。
宿主须先从已加载模板准备受支持的小资源包；不能暗中换默认母版或降级未知对象。

| 模板/草稿项目 | 上限 |
|---|---:|
| 包 source 页 / 追加实例 / 授权页型 | 各 64（scope 可进一步减小实例数） |
| 母版 / 媒体项 / 授权槽位 | 64 / 256 / 256 |
| native 形状总数 / source 资源总数 | 1,024 / 2,048 |
| 组深度 / native 序列化结构深度 | 8 / 32 |
| native 单集合 / 总节点 | 4,096 / 100,000 |
| native 单字符串 / 序列化前保守字节预算 | 128 KiB / 2 MiB |
| native 浮点数 | 有限且绝对值 ≤ 1e9；画布宽高 > 0 |
| 单媒体 / 包总媒体 / 复制后媒体总量 | 8 MiB / 32 MiB / 32 MiB |
| 图片宽高 / 固定 HTTPS URL | 各 4,096 px / 2,048 字节 |
| 完整草稿局部映射 / 全部槽位操作 | 4,096 / 256 |

文本与 wire 限制沿用上表。native 检查采用无分配的 serde visitor，先检查有限数、字符串、
节点和深度，再进行有界 JSON 引用发现；二进制媒体另计。拒绝资源错误不回显 native/响应正文。


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

`crates/ai/tests/fixtures/` 包含三页小文档、两页授权请求与固定响应。其中未授权字段带有标记，测试验证它们不会被上传。`fake` 模块仅在单元测试或 `test-support` feature 下可用；原始脚本响应走真实解码和边界校验，支持失败与耗尽，不持久化 prompt 或正文。新增 `template-response.json` 与原生合成对象测试资源闭包、复制实例、empty target、未知/越权引用、局部映射、媒体/深度/文字边界。`FakeProvider::respond_template` 走真实有界解码、授权校验与完整 draft bind；无网络。

```bash
cargo test --offline -p deckcraft-ai --features test-support
cargo test --offline -p deckcraft-engine
cargo test --offline -p xtask
cargo fmt --all -- --check
cargo clippy --offline --all-targets --no-deps -p deckcraft-engine -p deckcraft-ai --features deckcraft-ai/test-support -- -D warnings
cargo xtask layers
```

没有可截图的 UI 或实际安装/渲染后的修改结果，视觉验收不适用于这个只读契约组件；没有将进程退出成功当作图片检查。`cargo xtask wasm` 已把 AI 纯业务 crate 纳入检查集合，组件使用 `cargo xtask ci` 运行工作区门禁及实际 WASM；合并后的 P0 feature head 仍须重新独立验收。模板视觉/PPTX 回读属于 P4，不以局部测试替代。

P0 阶段已包含 3 套本地模板样本规范（`docs/ai-samples.md`）及离线元数据夹具（`samples-catalog.json`）；未确认许可的外部原模板库未直接提交到仓库。P0 全部契约与夹具已闭环。原子安装见 [P1](ai-apply.md)。P4 的实际填槽/渲染/PPTX 回读尚未实现，离线契约通过不等于真实模板视觉验收。

**关键决策**：将授权和提交白名单留在引擎边界，避免 provider 扩权；使用显式段落替换并保持数量，避免首版文本精简先损坏格式，再由历史/预览补救。
