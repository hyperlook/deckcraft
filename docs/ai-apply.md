# 接受一页提案

一轮能交给模型的依据只有四样：这份文稿的总大纲、用户点名的那一页、这一轮的意见、这次备好的素材。素材是已经摘出来的文字或图片说明，不是文件路径。用户自己在片子上改字，不写进大纲，也不使这一轮作废。

模型交回的是这一页的新标题和新要点，条数可以变。用户看完接受，才写入。接受做两件事，记成一步撤销：

- 换掉点名那一页的标题和正文。多出来的正文栏清空，避免旧说法留下。图、表格、组合和页脚不动。
- 大纲里这一页改成同一套标题和要点。大纲里的其他页不动。

点名用的是那一页的身份，不是屏幕上正看着的第几页，也不是当前打开的另一份文稿。这一页已经删掉、文稿已经关上，或者鼠标还按着在拖，就整次不写。内容没变就不再记一步撤销。撤销之后，同一版提案可以再接受一次。

改大纲、改素材本身只更新这两份依据，不增删页面。

```text
ai.setOutline     {docUid?, pages: [{pageId?, title, points}]}
ai.setMaterials   {docUid?, materials: [{name, kind?: text|image|file, body}]}
ai.accept         {docUid?, pageId, title, points, label?}
```

遮罩还没做。现在只在拖动未松开时拒绝写入。

```bash
cargo test --offline -p deckcraft-engine --lib ai_apply
cargo test --offline -p deckcraft-ai --lib
```
