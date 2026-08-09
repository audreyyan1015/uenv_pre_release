# 260804 · `frontend_add` 合入与 Hub 控制台浅色改造 联调记录

| 项 | 内容 |
|----|------|
| 目标分支 | `feature/worker-pool-260728_HubEpisodeStackRubric` |
| 合入来源 | `origin/frontend_add` @ `6dc5ebd` |
| 真机 | `8.130.95.176`（README §0.5 的 Hub 机），Ubuntu / Linux x86_64，4 vCPU / 16 GiB |
| 隔离方式 | 代码同步到 `/root/uenv-console-e2e`，独立数据目录 `/root/uenv-console-e2e-run`，监听 **18091**；未触碰 8088 与 `/root/uenv` |

---

## 0. 结论

| # | 项 | 结果 |
|---|----|------|
| 1 | `frontend_add` 合并 | 3 个文档冲突，代码 0 冲突；Hub 侧改动零丢失 |
| 2 | 本地 Hub workspace 测试 | **162 passed / 0 failed** |
| 3 | 本地主 workspace 测试 | **284 passed / 0 failed** |
| 4 | 真机 Hub workspace 测试 | **162 passed / 0 failed** |
| 5 | 真机主 workspace 测试 | 全部通过（含 `trajectory_upload_e2e`） |
| 6 | 控制台浅色改造 | 对齐 `frontend/src/styles.css` 的 `:root` 浅色令牌 |
| 7 | 真机渲染回归 | 13 条路由全部画出，无错误块 |
| 8 | WCAG AA 对比度审计 | 真机 311 处、本地 375 处文字/背景组合，**全部达标** |
| 9 | 生产 Hub `:8088` | 全程未受影响 |

---

## 1. 合并：冲突面比预想小得多

`origin/frontend_add` 相对合并基点 `233dac5` 有近百个提交、1458 个文件。
但真正与本分支重叠的只有 41 个文件，且全部是此前手工挪过来的 SWE-smith 那批
——Hub 目录完全不在重叠范围内。

实际冲突只有 3 个 Markdown 文档，且做忽略行尾空白的比对后，
**第一个文件的差异为零**，另外两个的差异恰好就是我方做的 Hub 状态更新：

```text
< | **Hub** | EnvPackage + Episode Stack 正式注册 | ✅ swe-bench-smith@0.1.0 …
> | **Hub** | EnvPackage 正式注册 | ⏳ 后置，不阻塞本期
```

也就是说，冲突的成因是我此前整合时顺手剥掉了上游用作 Markdown 换行的行尾双空格。
解决办法因此很直接：**以上游版本为底**（保留其排版意图，减少后续再冲突），
再把我方那几条 Hub 状态更新逐条贴回。

合并后用两条对照确认没有丢东西：

```bash
# 我方 Hub 改动是否完好 —— 输出为空即零丢失
git diff --stat <合并前 HEAD> -- uenv-hub/ scripts/ Docs/hub/

# 上游 Worker 改动是否并入 —— 9 个文件 467 插入
git diff --stat <合并前 HEAD> -- uenv-worker/src/
```

---

## 2. 合并暴露出的一个既有测试缺陷

合并后本地跑主 workspace，`uenv-worker` 有 3 个测试失败：

```text
episode::model_client::tests::includes_previous_action_and_evaluator_feedback_after_first_step
episode::model_client::tests::ignores_payload_generation_config_and_uses_default_config
episode::model_client::tests::uses_typed_generation_config_json
```

先确认归属：在 `origin/frontend_add` 原始提交上另开 worktree 跑同样的测试，
**同样三个失败**。所以不是合并引入的，是上游既有问题。

再看性质。这四个测试都用裸 `TcpListener` 起一个假的 LLM 服务，
`read()` **一次**就把收到的字节当成完整 HTTP 请求去断言：

```rust
let n = stream.read(&mut buffer).await.expect("read");
let request = String::from_utf8_lossy(&buffer[..n]).to_string();
```

单次 `read()` 只能拿到内核当次交付的字节，header 与 body 是否落在同一个 TCP 段
由协议栈决定。规律因此很清楚：**四个测试里唯一只断言 header 的那个是通过的**，
三个断言 body 的全部失败。它们实际上是在断言操作系统的分段行为
——Linux 上常合并所以碰巧过，macOS 上常拆开所以必挂。

改法是让假服务读满 `Content-Length` 再断言（`read_full_request`），
**产品代码一行未动**。验证：

| 平台 | 修复前 | 修复后 |
|------|--------|--------|
| macOS aarch64 | 8 passed / 3 failed | **11 passed / 0 failed** |
| Linux x86_64（真机） | 11 passed / 0 failed | **11 passed / 0 failed** |

真机上修复前后都是全过，说明这次改动没有把原本能过的地方弄坏，
只是把"碰巧能过"变成了"确定能过"。

> 真机跑主 workspace 时另有一个 `worker_uploader_to_real_server_e2e` 失败，
> 原因是它依赖 `target/debug/uenv-adapter-core` 这个二进制而当时尚未构建
> （测试自己在 panic 里写明了）。`cargo build -p uenv-adapter-core` 后即通过。

---

## 3. 浅色改造：不是把颜色取反

控制台原先镜像的是 `frontend/src/styles.css` 里的 `.dark`。
前端本来就定义了完整的 `:root` **浅色**调色板，所以改造的主体就是换个镜像对象，
`--background` / `--card` / `--border` / `--primary` / `--success` / `--warning` /
`--info` / `--pending` 逐条对齐。样式表本身几乎全部走令牌，
`:root` 之外只有一处硬编码颜色（toast 阴影）。

但有三类地方不是换令牌就能了事：

**其一，白色混色是深色主题的惯用法。** `color-mix(…, white)` 在深底上表示"提亮以强调"，
浅底上方向恰好反了。抽出 `--contrast-ink` 表示"提升对比的方向"，三处硬编码 `white` 全部改走它。

**其二，状态色不能既当填充又当文字。** 浅底上 `--warning`（L=0.7）作文字只有约 2.2:1，
远低于可读阈值，但它当徽章底色是合适的。因此另立 `--success-ink` / `--warning-ink` /
`--danger-ink` / `--info-ink`（L≈0.46–0.48，对白底约 5:1）：徽章文字与 JSON 语法高亮走 ink 变体，
描边与底色仍走基色。

**其三，浅色下白卡叠在近白页面上会发平。** 深色主题靠明度差自然分面，浅色主题得靠投影，
所以补了 `--shadow-sm` / `--shadow-md` 两级。

---

## 4. 对比度不能靠看截图判断

改完之后截图看着"还行"，但"还行"不是结论。为此写了
`scripts/hub-console-contrast-audit.mjs`：连上 Chrome 的调试端口，
在页面里遍历每个可见文字节点，取 `getComputedStyle` 的实际前景色、
沿祖先链合成出实际背景色，按 WCAG 2.1 算对比度并对照 AA 阈值
（普通文字 4.5:1，大号文字 3:1）。

有一个坑值得记：Chrome 的 `getComputedStyle` 会把 `oklch()` / `color-mix()`
**原样保留**在计算值里，用正则解析 `rgb(...)` 会一个都匹配不到——
首次运行时"取样 0 处"却报了绿，是典型的假阳性。
改成把颜色丢给 canvas 光栅化一个像素、直接读 sRGB 字节才拿到真值。

首轮实测在 13 条路由上抓到 21 处不达标，去重后只有两类，且都出在**选中**的导航项上：

| 元素 | 实测 | 需要 | 成因 |
|------|------|------|------|
| `.nav-item.active .ico` | 3.58:1 | 4.5 | 图标用 `--primary`，却落在 22% 主色淡蓝底上，不是白底 |
| `.nav-item.active em` | 4.39:1 | 4.5 | 同上，计数沿用了白底的 `--muted-foreground` |

修法是给选中态单独取色（新增 `--primary-ink`，计数改用更深的前景混色）。
复测：真机 13 条路由 311 处取样、本地 16 条路由 375 处取样，**全部达标**。

审计已接进 `scripts/verify-hub-console-e2e.sh`，作为渲染回归之后的一道门。
为确认它不是摆设，做了一次变异测试：把 `--muted-foreground` 调亮到 L=0.86 后重跑，
立刻报出 9 处不达标（低至 1.29:1）；还原后恢复全绿。

---

## 5. 真机复现步骤

```bash
# 1) 同步（真机无 git 凭据）
rsync -az --delete --exclude '.git/' --exclude 'target/' --exclude 'data/' \
  ./ root@8.130.95.176:/root/uenv-console-e2e/

# 2) 真机构建与测试
ssh root@8.130.95.176 'cd /root/uenv-console-e2e/uenv-hub && cargo test --workspace'
ssh root@8.130.95.176 'cd /root/uenv-console-e2e && cargo build -p uenv-adapter-core && cargo test --workspace'

# 3) 起一个与生产 :8088 隔离的实例（独立 db / artifact 目录，监听 18091）
ssh root@8.130.95.176 'bash /root/hub-e2e-up.sh'

# 4) 真机侧 API 联调
ssh root@8.130.95.176 \
  'cd /root/uenv-console-e2e && UENV_HUB_CONSOLE_BASE=http://127.0.0.1:18091 \
   bash scripts/verify-hub-console-e2e.sh'

# 5) 渲染与对比度要本机 Chrome（真机无浏览器），走隧道打真机数据
ssh -f -N -L 18093:127.0.0.1:18091 root@8.130.95.176
UENV_HUB_CONSOLE_BASE=http://127.0.0.1:18093 bash scripts/verify-hub-console-e2e.sh
```

> 真机 Worker 构建需要 `protobuf-compiler` / `pkg-config` / `libssl-dev`，
> 该机原先没装，本次已补齐（`libprotoc 3.21.12`）。
>
> 隧道早先用 `expect` 维持不住（`ssh -N` 无输出，后台 expect 会话一散隧道就断，
> 截出来是空白页）。已在该机装好 SSH 公钥，改用普通 `ssh -f -N -L`，稳定。

---

## 6. 真机实测数据

`/api/v1/system/overview` 在 Linux 上 `/proc` 派生字段全部有真值
（本机 macOS 上这些字段一律显示"—"，因为无 `/proc`）：

```text
CPU 使用率  0.1%       4 核 · linux/x86_64
内存占用    4.8%       738 MiB / 15.0 GiB
负载 (1m)   0.08       5m 0.56 · 15m 0.34
Hub 进程内存 30.6 MiB  RSS

产物库占用  626 KiB    29 个文件
数据库文件  4.0 KiB    sqlite:///root/uenv-console-e2e-run/hub.db
注册内容    环境 5 · 环境包 6 · Stack 3 · Agent Bridge 2 · 模板 5
```

生产 Hub 全程未受影响（本次联调期间 `:8088` 未在运行，
隔离实例使用独立端口、独立数据库与独立产物目录，不存在共享状态）。
