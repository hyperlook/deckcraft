# P1：引擎整体提交与单条历史

关联：[#1](https://github.com/hyperlook/deckcraft/issues/1)、[P1 #7](https://github.com/hyperlook/deckcraft/issues/7)。设计依据：[AI 助手 RFC](rfcs/ai-assistant.md) 第 6 节。契约输入见 `crates/engine::ai_contract`。

这是引擎提交边界，不是 AI 面板、模型或请求池。没有网络、文件保存或导出。视觉验收和 PPTX 回读仍属于 P4。

## 入口

```text
宿主冻结的 TextScope + TextPatch，或已 bind 的 AppendDraft
  │ 文档 UID，不看活动标签或页序号
  ▼
Session::apply_ai
  │ 重复 job_id → 返回已记录结果，不写文档
  │ 指针交互未结束 → 拒绝，不结束交互、不写历史
  │ 内容版本不符 → 冲突，不写账本
  ▼
独立候选副本上重检契约并构造完整结果
  │ 任一步失败或 panic → 丢弃候选
  ▼
一次安装文档与最终选区，一条动态命名历史，记入幂等账本
```

文本也可以走命令 `ai.applyPatch`，参数是 `{label?, scope, patch}`。命令不进入会话 journal，也不由命令表再写一条撤销记录。模板草稿持有原生字节，不能从 JSON 重建，只走 `AiCommit::Append`。

## 内容身份

`DocState::content_revision` 从 1 开始，只增不减。`Session::edit`、撤销、重做、恢复已保存副本，以及文本框自适应和自动更正这些直接替换文档的路径会推进它。`select`、保存标记和只切换活动文档不会。

因此任务期间的改字、删页、调序、撤销和重做都会使旧 `base_content_revision` 冲突。只改选区不会。撤销不会把内容版本拨回旧数字，所以撤销后的文档即使看起来和快照时一样，也不能用旧快照再提交。

## 历史与无变化

成功且有实际变化时，历史保存**提交前**的文档和选区，标签用调用方给出的名称。空标签生成为 `AI: 更新 N 处文本` 或 `AI: 追加 N 页`。标签最长 256 个 Unicode 标量值，拒绝控制字符。

文本替换按稳定 `(SlideId, ShapeId)` 定位，保留段落数、段落属性和首个 run 的格式，不调用有损 `text.set`。文字相同的段落保持原 run，不制造虚假变化。

没有实际变化时不写历史，也不清空 Redo，但仍把该 `job_id` 记为已处理。之后的重复提交返回同一无变化结果。

## 幂等账本

账本挂在文档会话上，不放进演示文稿，也不进撤销栈。每个文档最多 256 个已成功 `job_id`。满了以后拒绝新任务，不淘汰旧记录；淘汰会让晚到重试重新加页。

重复 `job_id` 在冲突检查和交互检查之前返回已应用结果。用户撤销之后，同一任务重试不会再加页或再写历史。重新生成必须使用新 `job_id`。失败和冲突不入账，以便修正后的新任务可以提交。

## 模板追加

安装前重跑 `draft.validate()` 和 `validate_target`。每个 local resource 分配新的目标 ID，不把草稿局部 ID 或样本 ID 写进目标文档。每个实例复制自己的母版、被选中的那一个布局、形状和内嵌媒体；页链接只按该实例的 `PageLink` 重写。缺映射、无法确认的链接或复制后仍指向样本/旧页的引用会使候选失败，不安装。

空目标采用资源包的画布、默认文字样式和页脚。这三项若含媒体或页链接则拒绝，不为全局样式猜测副本。非空目标不改旧页、旧母版和全局样式。

## 失败与 panic

普通校验错误发生在安装之前。`apply_ai` 跑在现有 panic guard 内；guard 现在快照全部打开文档的文档、选区、历史、内容版本、交互和账本。panic 恢复这些字段，不把未完成的指针交互清掉。

未结束的指针拖动（目标文档的 `interaction`，或活动文档上尚未松开的拖动）返回“交互进行中”。宿主应等手势结束再重试，届时重新比较内容版本。打开的文本光标不是未提交事务，不阻止提交。

## 明确未做

没有 UI、模型适配、请求池、取消协调或单页重试。没有把模板样本渲染或导出 PPTX。CLI、控制通道和 MCP 复用这一入口是 P6。空目标全局样式里的资源链接本阶段拒绝，不在安装时猜测目的地。

```bash
cargo test --offline -p deckcraft-engine --lib ai_apply
cargo test --offline -p deckcraft-engine --lib
cargo fmt -p deckcraft-engine -- --check
cargo clippy --offline -p deckcraft-engine --all-targets --no-deps -- -D warnings
```

没有可截图的面板。模板测试检查 ID、链接、媒体字节和撤销，不把测试通过当作视觉验收。
