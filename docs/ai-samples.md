# P0：离线样本模板规范与元数据目录

关联：[#1](https://github.com/hyperlook/deckcraft/issues/1)、[P0 #3](https://github.com/hyperlook/deckcraft/issues/3)、[P0 契约文档](ai-contracts.md)。设计依据：[AI 助手 RFC](rfcs/ai-assistant.md)。

本文档记录 P0 阶段选定的 3 套低资源、文字与原生矢量几何排版为主的模板样本规范，供后续 P4（模板执行器与物化）消费。

## 资产隔离与版权防护铁律（License & Provenance Policy）

- **许可待确认**：本规范引用的 3 套样本来源于本机外部技能资产库（`/Users/look/.pi/agent/skills/ppt-parallel/templates`），其原始开源或商业许可尚未获得书面确认。
- **严禁向 Git 提交原资产**：**绝不**将原始源脚本（`.js`）、第三方参考 `.pptx` 或转换后的 `.deckcraft` 二进制资产提交到 Git 仓库中。
- **纯语义元数据在库**：代码库仅保留纯语义元数据（见 `crates/ai/tests/fixtures/samples-catalog.json`）与契约测试。
- **运行时路径可配置**：宿主加载真实工作副本时，统一从外部环境变量 `DECKCRAFT_TEMPLATE_DIR` 或宿主指定配置路径加载，不假设模板文件与仓库硬编码绑定。

---

## 选定的 3 套低资源原生样本

经全库结构审计与槽位核验（结构健全、0 外部图片依赖、0 缺失字体、文字与原生形状排版密集），选定以下 3 套基准模板：

### 1. `garyvee_content_strategy_framework`（营销/框架类）
- **定位**：强视觉冲击力、大字号标题、卡片化布局与流程步骤说明。
- **尺寸与画幅**：`720 x 405` (16:9)
- **页面总数**：17 页
- **图片槽位**：0（纯原生矩形/形状背景与矢量卡片，无外部图片依赖）
- **文字槽位**：185 个
- **源文件 SHA-256**：`45a1619662039f67bd8f830f16f992ce735996a60458fb6af596703fa8c53cfa`
- **原生底稿 SHA-256**：`f72d5f76a8327f430d95da1638eb3217d475c2d1d1e978679e741ea96f1acc81`
- **主要字体**：Arial Black, Arial（系统常规字体）
- **典型页型与角色（Family Roles）**：
  - `cover`（Slide 0 / s01）：醒目主标题与模型总览。
  - `agenda`（Slide 1 / s02）：多模块内容框架目录。
  - `concept`（Slide 2–5 / s03–s06）：核心支柱内容（Pillar Content）概念解析。
  - `process`（Slide 6–11 / s07–s12）：微内容衍生流转步骤。
  - `summary`（Slide 12–16 / s13–s17）：分发矩阵与执行行动指南。

### 2. `crypto_quantum_risk_briefing`（安全简报类）
- **定位**：结构严谨的研讨简报，包含威胁时间线、矩阵对比与防御框架。
- **尺寸与画幅**：`960 x 540` (16:9)
- **页面总数**：15 页
- **图片槽位**：0（无外部图片依赖）
- **文字槽位**：225 个
- **源文件 SHA-256**：`590ca07e2ed016870ed5e44fc235ee786ccc073e7d6e2ea900c5a3cc5c469be0`
- **原生底稿 SHA-256**：`11fe2b1c8df04a44b96f54efdef5b4a61f676011096ace70278386dd74d8c2f3`
- **主要字体**：Arial
- **典型页型与角色（Family Roles）**：
  - `cover`（Slide 0 / s01）：简报编号与密级风格标题。
  - `executive_summary`（Slide 1–3 / s02–s04）：核心态势研判与结论摘要。
  - `threat_matrix`（Slide 4–7 / s05–s08）：密码学算法脆弱性分类矩阵。
  - `mitigation`（Slide 8–11 / s09–s12）：后量子抗抵防御框架与迁移路径。
  - `recommendations`（Slide 12–14 / s13–s15）：路线图检查单与阶段里程碑。

### 3. `fixed_income_macro_outlook`（宏观研报类）
- **定位**：专业金融分析版式，多栏关键指标、收益率曲线分析框架与资产配置。
- **尺寸与画幅**：`960 x 540` (16:9)
- **页面总数**：15 页
- **图片槽位**：0（无外部图片依赖）
- **文字槽位**：312 个
- **源文件 SHA-256**：`2ae8ac1cf21d2615c7ac2d352096d141bac75bbe0dd1d44370d705a246e4f7f7`
- **原生底稿 SHA-256**：`0728683e5f0b897786b19a992e9771f1c95429da68404c37272cca694ebced3e`
- **主要字体**：Arial
- **典型页型与角色（Family Roles）**：
  - `cover`（Slide 0 / s01）：宏观经济研报封面与机构副标。
  - `key_takeaways`（Slide 1 / s02）：三大核心投资观点摘要。
  - `indicators`（Slide 2–6 / s03–s07）：多栏宏观指标对比与数据分析。
  - `policy`（Slide 7–10 / s08–s11）：货币政策、通胀预期与央行资产负债表。
  - `allocation`（Slide 11–14 / s12–s15）：组合久期与信用利差配置建议。

---

## 槽位与容量策略（Slot & Budget Policy）

1. **原样版面贴合**：`contentBudget.policy` 为 `fit-to-original-layout`。
2. **字符预算**：以原模板槽位的字符数为基准；中文替换不得超出原布局安全容量（由 P4 布局检查器最终判定）。
3. **段落数保真**：严格继承 P0 文本契约，段落数量必须与原对象相等，避免段落增删引发垂直溢出。
4. **多页生成映射**：大纲生成（P5）根据卡片意图匹配上述 `role`，抽取对应的单页布局实例，填入经过校验的语义段落。

---

## 阶段衔接

- **P0（当前）**：完成样本元数据记录、SHA 校验夹具与离线解码测试；
- **P1**：提供原子应用与资源重映射支持；
- **P4**：基于 `DECKCRAFT_TEMPLATE_DIR` 真实物化这 3 套模板的工作副本，进行填槽、中文排版、图像渲染与 PPTX 往返测试。
