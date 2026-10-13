# 任务收尾 — 文档更新决策

## Status

- Status: **Ops**
- 最近审阅：2026-10-13
- 索引：[`../README.md`](../README.md)
- 分层原则：[`../README.md` 维护约定](../README.md)

---

## 1. Current 门槛（严）

**写入 / 改写 `docs/current/` 仅当同时满足：**

1. 行为已合入主开发分支（或用户明确视为可依赖）；且  
2. 其他端 / 运维 / 帮助文案可以按该描述依赖（非「待联调 / 待现网观察」）。

代码已合、验收未完 → **只改绑定 Roadmap**（勾选 +「已落地，待验收」），**不**把未验收行为写成 Current 现状。

Draft /「不实现」的 Roadmap **永不**当作 Current 依据。

---

## 2. 决策表（任务类型 → 最少更新集）

| 任务类型 | 必更 | 按需 | 不要动 |
|----------|------|------|--------|
| Roadmap 条目落地（有绑定文档） | 该 Roadmap：勾验收、Status/最近审阅、changelog 一行 | **仅当 §1 门槛满足** → 对应 `current/` | 无关 Roadmap；未验收当 Current |
| 纯修 bug / 重构（对外行为不变） | 无 | 相关 Roadmap「已知问题」划掉；CI 新坑 → [`ci-failure-log.md`](./ci-failure-log.md) | 为改而改 Current |
| 行为/契约变更且已达 §1 | 对应 `current/` + 文首审阅日期 | 原 Roadmap → 剩余缺口 / Superseded / Archive | 只改 Roadmap 不改 Current |
| 构建 / 部署 / 发布流程 | 对应 `ops/` | `script/README`、[`release-version-bump.md`](./release-version-bump.md) | Current 产品语义 |
| CI / 编译新失败 | [`ci-failure-log.md`](./ci-failure-log.md) 追加 + §3 | — | 当功能文档写 |
| 仅立项 / TODO（未实现） | 新建或更新 Roadmap；Status=Draft/Roadmap | `docs/README` 索引加一行 | Current、产品 i18n 帮助 |
| 结项归档 | Roadmap 缩成剩余缺口或迁 `archive/`；**Current 按 §1 收口** | 索引迁移说明 | 继续把 Archive 当规划 SoT |
| 纯文档错别字 / 链接 | 该篇即可 | — | 免本表形式主义 |

**绑定文档** = 开干时指定的那篇 Roadmap/Ops。未指定时：Agent 提 1–2 个候选，确认后再改；禁止散改多篇「可能相关」的 Current。

---

## 3. 怎么更新（四步）

1. **定类型**（上表一行）。  
2. **改绑定文**：Status、最近审阅、勾选或「代码已落地，待联调/现网」；必要时文末 changelog 一行。  
3. **Current 门禁**：未过 §1 → 停；过了 → 只改与本次行为对应的 Current 段落，并更新日期。  
4. **索引**：仅新文档或分区迁移时改 [`docs/README.md`](../README.md)；日常勾选不动索引。

文首保持：`Status` + 最近审阅日期。

---

## 4. 正反例

| 场景 | 正确 | 错误 |
|------|------|------|
| §9 S1–S9 代码合入，待现网观察 | Roadmap 勾选 +「待现网」；**不**改 `web-managed-config` 除非已有可依赖表述需修正 | 把 S4 索引写成 Current「已如何如何」且不提待观察 |
| MagicDNS 通配语义已合并且帮助可引用 | 更新 `current/magic-dns.md`；Roadmap 剩余缺口或 Archive | 只在 Roadmap 写「已落地」而 Current 仍是旧语义 |
| clippy 新规则导致 ET Test 红 | 追加 `ci-failure-log` | 写进某篇功能 Roadmap 当需求 |

---

## 5. 与提交前检查的分工

| 时机 | 文档 |
|------|------|
| 任务宣称完成 / 准备交代码 | **本文**（该不该写、写哪层） |
| 提交 / push / 开 PR / ET Test | [`ci-failure-log.md`](./ci-failure-log.md)（会不会再红） |
