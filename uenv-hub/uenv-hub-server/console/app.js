/*
 * UEnv Hub 控制台。
 *
 * 无框架、无构建步骤：控制台随 uenv-hub-server 二进制一起分发，部署机上只需要
 * 有 Hub 本身，不需要 Node 工具链。所有数据都来自 Hub 自己的 REST API，页面不
 * 保存任何派生副本，刷新即以服务端为准。
 */
(() => {
  "use strict";

  // ------------------------------------------------------------------ 工具

  const $ = (sel) => document.querySelector(sel);
  const el = (tag, attrs, children) => {
    const node = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v === null || v === undefined || v === false) continue;
      if (k === "class") node.className = v;
      else if (k === "html") node.innerHTML = v;
      else if (k === "text") node.textContent = v;
      else if (k.startsWith("on") && typeof v === "function") node.addEventListener(k.slice(2), v);
      else node.setAttribute(k, v === true ? "" : String(v));
    }
    for (const c of [].concat(children || [])) {
      if (c === null || c === undefined || c === false) continue;
      // 任何非 Node 的值（数字、布尔、对象）都降级成文本：一个字段的类型意外
      // 不该让整页 appendChild 抛错、白屏。
      if (c instanceof Node) node.appendChild(c);
      else if (typeof c === "object") node.appendChild(document.createTextNode(JSON.stringify(c)));
      else node.appendChild(document.createTextNode(String(c)));
    }
    return node;
  };

  const esc = (s) =>
    String(s ?? "").replace(
      /[&<>"']/g,
      (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
    );

  const fmtBytes = (n) => {
    if (n === null || n === undefined) return "—";
    if (n < 1024) return `${n} B`;
    const units = ["KiB", "MiB", "GiB", "TiB"];
    let v = n / 1024;
    let i = 0;
    while (v >= 1024 && i < units.length - 1) {
      v /= 1024;
      i += 1;
    }
    return `${v.toFixed(v >= 100 ? 0 : 1)} ${units[i]}`;
  };

  const fmtNum = (n) => (n === null || n === undefined ? "—" : Number(n).toLocaleString("en-US"));

  const fmtDuration = (secs) => {
    if (secs === null || secs === undefined) return "—";
    const s = Math.max(0, Math.floor(secs));
    const d = Math.floor(s / 86400);
    const h = Math.floor((s % 86400) / 3600);
    const m = Math.floor((s % 3600) / 60);
    if (d > 0) return `${d}d ${h}h`;
    if (h > 0) return `${h}h ${m}m`;
    if (m > 0) return `${m}m ${s % 60}s`;
    return `${s}s`;
  };

  const fmtTime = (epochSecs) => {
    if (!epochSecs) return "—";
    const d = new Date(epochSecs * 1000);
    const p = (n) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
  };

  const shortDigest = (d) => {
    if (!d) return "—";
    const hex = String(d).replace(/^sha256:/, "");
    return hex.length > 20 ? `sha256:${hex.slice(0, 12)}…${hex.slice(-6)}` : String(d);
  };

  /** 语法高亮的 JSON 视图。输入先转义，再对 token 着色，避免 XSS。 */
  const jsonBlock = (value) => {
    const text = esc(JSON.stringify(value, null, 2) ?? "null");
    const html = text.replace(
      /("(\\u[a-zA-Z0-9]{4}|\\[^u]|[^\\"])*"(\s*:)?|\b(true|false)\b|\bnull\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)/g,
      (match) => {
        let cls = "tok-num";
        if (/^"/.test(match)) cls = /:$/.test(match) ? "tok-key" : "tok-str";
        else if (/true|false/.test(match)) cls = "tok-bool";
        else if (/null/.test(match)) cls = "tok-null";
        return `<span class="${cls}">${match}</span>`;
      },
    );
    return el("pre", { class: "json", html });
  };

  const disclose = (label, node, open) =>
    el("details", { class: "disclose", open: !!open }, [
      el("summary", { text: label }),
      el("div", {}, [node]),
    ]);

  const card = (title, body, opts = {}) => {
    const header = el("header", {}, [el("h3", { text: title })]);
    if (opts.hint) header.appendChild(el("span", { class: "hint spacer", text: opts.hint }));
    for (const action of opts.actions || []) {
      if (!header.querySelector(".spacer")) action.classList.add("spacer");
      header.appendChild(action);
    }
    return el("section", { class: "card" }, [
      header,
      el("div", { class: opts.tight ? "card-body tight" : "card-body" }, [body]),
    ]);
  };

  const kv = (pairs) => {
    const dl = el("dl", { class: "kv" });
    for (const [k, v] of pairs) {
      if (v === undefined) continue;
      dl.appendChild(el("dt", { text: k }));
      const cell = v instanceof Node ? v : el("span", { text: v === null ? "—" : String(v) });
      dl.appendChild(el("dd", {}, [cell]));
    }
    return dl;
  };

  const table = (columns, rows, opts = {}) => {
    if (!rows.length) return el("div", { class: "empty", text: opts.empty || "暂无数据" });
    const thead = el(
      "thead",
      {},
      [el("tr", {}, columns.map((c) => el("th", { text: typeof c === "string" ? c : c.label })))],
    );
    const tbody = el("tbody");
    for (const row of rows) {
      const tr = el("tr", row.onclick ? { class: "clickable", onclick: row.onclick } : {});
      row.cells.forEach((cell, i) => {
        const col = columns[i];
        const cls = typeof col === "object" && col.num ? "num" : null;
        tr.appendChild(
          el("td", { class: cls }, [typeof cell === "string" ? document.createTextNode(cell) : cell]),
        );
      });
      tbody.appendChild(tr);
    }
    return el("table", { class: "tbl" }, [thead, tbody]);
  };

  const badge = (text, kind) => el("span", { class: kind ? `badge ${kind}` : "badge", text });

  const link = (text, href, cls) => el("a", { class: cls || "", href, text });

  /// Stack 里的 agent_scaffold 是个结构体（package_id / version / agent_kind），
  /// 渲染成指向该环境包详情的可点引用，而不是一坨 JSON。
  const scaffoldRef = (ref) => {
    if (!ref) return "—";
    if (typeof ref === "string") return ref;
    const id = ref.package_id || ref.id;
    if (!id) return JSON.stringify(ref);
    const label = `${id}@${ref.version || "latest"}`;
    const node = el("span", {}, [
      link(label, `#/packages/${encodeURIComponent(id)}`, "mono"),
    ]);
    if (ref.agent_kind) node.appendChild(badge(ref.agent_kind, "info"));
    return node;
  };

  const toast = (message, kind) => {
    const node = el("div", { class: kind ? `toast ${kind}` : "toast", text: message });
    $("#toasts").appendChild(node);
    setTimeout(() => node.remove(), kind === "bad" ? 7000 : 3500);
  };

  // -------------------------------------------------------------- API 客户端

  const TOKEN_KEY = "uenv-hub-console-token";

  const store = {
    get token() {
      return localStorage.getItem(TOKEN_KEY) || "";
    },
    set token(v) {
      if (v) localStorage.setItem(TOKEN_KEY, v);
      else localStorage.removeItem(TOKEN_KEY);
    },
  };

  /**
   * Hub API 调用。控制台与 Hub 同源，因此不需要配置端点。
   *
   * 错误按 Hub 的统一错误信封（`{ error: { code, message }, request_id }`）
   * 还原，把 `request_id` 一并带出，方便对着服务端日志排查同一次请求。
   */
  async function api(path, opts = {}) {
    const headers = { Accept: "application/json", ...(opts.headers || {}) };
    if (store.token) headers.Authorization = `Bearer ${store.token}`;
    const resp = await fetch(path, { ...opts, headers });
    if (resp.status === 204) return null;
    const text = await resp.text();
    let body = null;
    try {
      body = text ? JSON.parse(text) : null;
    } catch {
      body = null;
    }
    if (!resp.ok) {
      const err = new Error(
        body?.error?.message || `${resp.status} ${resp.statusText}` || "请求失败",
      );
      err.status = resp.status;
      err.code = body?.error?.code;
      err.requestId = body?.request_id || resp.headers.get("x-request-id");
      throw err;
    }
    return body;
  }

  const qs = (params) => {
    const sp = new URLSearchParams();
    for (const [k, v] of Object.entries(params)) {
      if (v !== null && v !== undefined && v !== "") sp.set(k, v);
    }
    const s = sp.toString();
    return s ? `?${s}` : "";
  };

  // -------------------------------------------------------- 环境包的角色分类
  //
  // Hub 存的「环境包」是一种分发单元，不是一类东西：任务数据、Agent 脚手架、
  // 评分契约和纯镜像 tar 都用同一张表。服务端按 manifest 推导 `kind`，控制台据此
  // 分栏，这样导航反映的是构件的角色，而不是数据库的表结构。

  const PACKAGE_KINDS = {
    benchmark: { label: "基准数据集", badge: "info", desc: "任务目录 + 评测规格，喂给某个环境契约" },
    agent_scaffold: { label: "Agent 脚手架", badge: "ok", desc: "决定「怎么答」，由 Agent 宿主同步" },
    rubric: { label: "评分契约", badge: "warn", desc: "判分口径与对齐证据" },
    image_bundle: { label: "镜像包", badge: "", desc: "只有 docker load 输入，无任务数据" },
    fixture: { label: "测试夹具", badge: "", desc: "回归用，不参与正式训练" },
    other: { label: "其他", badge: "", desc: "未声明角色" },
  };

  const kindMeta = (k) => PACKAGE_KINDS[k] || PACKAGE_KINDS.other;

  /** 一次拉满整张包表并按 kind 归组；列表端点不支持按角色过滤，故在前端分。 */
  async function packagesByKind() {
    const data = await api(`/api/v1/packages${qs({ per_page: 200 })}`);
    const groups = {};
    for (const p of data.items || []) {
      const k = p.kind || "other";
      (groups[k] = groups[k] || []).push(p);
    }
    return { total: data.total, items: data.items || [], groups };
  }

  // ------------------------------------------------------------------ 路由

  const routes = {};
  const state = { overview: null, timer: null, currentRender: null };

  const parseHash = () => {
    const raw = (location.hash || "#/overview").replace(/^#\/?/, "");
    const [pathPart, queryPart] = raw.split("?");
    const segs = pathPart.split("/").filter(Boolean).map(decodeURIComponent);
    return { segs, query: Object.fromEntries(new URLSearchParams(queryPart || "")) };
  };

  const go = (hash) => {
    location.hash = hash;
  };

  function setCrumbs(title, sub) {
    const node = $("#crumbs");
    node.textContent = title;
    if (sub) node.appendChild(el("small", { text: sub }));
  }

  function loading() {
    const view = $("#view");
    view.replaceChildren(
      el("div", { class: "grid cols-3" }, [
        el("div", { class: "skeleton" }),
        el("div", { class: "skeleton" }),
        el("div", { class: "skeleton" }),
      ]),
    );
  }

  function renderError(error) {
    const parts = [`${error.code ? `[${error.code}] ` : ""}${error.message}`];
    if (error.status === 401 || error.status === 403) {
      parts.push("— 请在「连接与凭据」中填入具备相应角色的 API Token。");
    }
    if (error.requestId) parts.push(`(request_id: ${error.requestId})`);
    return el("div", { class: "err", text: parts.join(" ") });
  }

  /** 渲染一个视图：统一处理 loading / 异常，避免每个视图各写一遍。 */
  async function mount(builder) {
    const token = Symbol("render");
    state.currentRender = token;
    loading();
    let node;
    try {
      node = await builder();
    } catch (error) {
      node = renderError(error);
      if (error.status !== 401 && error.status !== 403) {
        console.error(error);
      }
    }
    // 期间用户可能已经切走，丢弃过期结果。
    if (state.currentRender !== token) return;
    $("#view").replaceChildren(node);
  }

  async function router() {
    const { segs, query } = parseHash();
    const name = segs[0] || "overview";
    const handler = routes[name] || routes.overview;
    for (const item of document.querySelectorAll(".nav-item")) {
      item.classList.toggle("active", item.dataset.route === name);
    }
    await mount(() => handler(segs.slice(1), query));
  }

  // -------------------------------------------------------------- 视图：总览

  function meterClass(pct) {
    if (pct === null || pct === undefined) return "";
    if (pct >= 90) return "bad";
    if (pct >= 75) return "warn";
    return "ok";
  }

  function statTile(label, value, sub, meter) {
    const children = [
      el("span", { class: "label", text: label }),
      el("span", { class: "value", text: value }),
    ];
    if (sub) children.push(el("span", { class: "sub", text: sub }));
    if (meter && meter.pct !== null && meter.pct !== undefined) {
      children.push(
        el("div", { class: `meter ${meterClass(meter.pct)}` }, [
          el("i", { style: `width:${Math.max(0, Math.min(100, meter.pct)).toFixed(1)}%` }),
        ]),
      );
    }
    return el("div", { class: "stat" }, children);
  }

  routes.overview = async () => {
    setCrumbs("总览");
    const ov = await api("/api/v1/system/overview");
    state.overview = ov;
    applyOverviewChrome(ov);

    const r = ov.registry;
    const h = ov.host;
    const s = ov.storage;

    const memUsedPct =
      h.memory_total_bytes && h.memory_available_bytes !== undefined
        ? ((h.memory_total_bytes - h.memory_available_bytes) / h.memory_total_bytes) * 100
        : null;
    const loadPct =
      h.load_average && h.cpu_cores ? (h.load_average[0] / h.cpu_cores) * 100 : null;

    const frag = document.createDocumentFragment();

    frag.appendChild(
      el("div", { class: "title-row" }, [
        el("h2", { text: "Hub 运行态总览" }),
        badge(ov.db_up ? "数据库正常" : "数据库不可用", ov.db_up ? "ok" : "bad"),
        badge(`已运行 ${fmtDuration(ov.uptime_seconds)}`, "info"),
        el("p", {
          text: "以下数字全部由 Hub 在请求时从数据库与文件系统实测得到，不是缓存的估计值。",
        }),
      ]),
    );

    // 主机资源
    const hostSection = el("div", { class: "section" }, [
      el("h4", { class: "section-title", text: "主机资源" }),
      el("div", { class: "grid cols-4" }, [
        statTile(
          "CPU 使用率",
          h.cpu_usage_percent === undefined || h.cpu_usage_percent === null
            ? "—"
            : `${h.cpu_usage_percent.toFixed(1)}%`,
          `${h.cpu_cores ?? "?"} 核 · ${h.os}/${h.arch}`,
          { pct: h.cpu_usage_percent ?? null },
        ),
        statTile(
          "内存占用",
          memUsedPct === null ? "—" : `${memUsedPct.toFixed(1)}%`,
          h.memory_total_bytes
            ? `${fmtBytes(h.memory_total_bytes - h.memory_available_bytes)} / ${fmtBytes(h.memory_total_bytes)}`
            : "非 Linux 主机不提供",
          { pct: memUsedPct },
        ),
        statTile(
          "负载 (1m)",
          h.load_average ? h.load_average[0].toFixed(2) : "—",
          h.load_average ? `5m ${h.load_average[1].toFixed(2)} · 15m ${h.load_average[2].toFixed(2)}` : "非 Linux 主机不提供",
          { pct: loadPct },
        ),
        statTile(
          "Hub 进程内存",
          fmtBytes(h.process_resident_bytes),
          h.process_resident_bytes === undefined || h.process_resident_bytes === null
            ? "非 Linux 主机不提供"
            : "RSS",
        ),
      ]),
    ]);
    frag.appendChild(hostSection);

    // 存储足迹
    frag.appendChild(
      el("div", { class: "section" }, [
        el("h4", { class: "section-title", text: "存储足迹" }),
        el("div", { class: "grid cols-4" }, [
          statTile("产物库占用", fmtBytes(s.artifact_bytes), `${fmtNum(s.artifact_files)} 个文件`),
          statTile(
            "登记产物字节",
            fmtBytes(r.package_artifact_bytes),
            `${fmtNum(r.package_artifacts)} 条产物记录`,
          ),
          statTile("数据库文件", fmtBytes(s.database_bytes), s.database_url),
          statTile(
            "产物目录",
            s.artifact_dir_exists ? "已就绪" : "未创建",
            s.artifact_dir,
          ),
        ]),
        s.artifact_bytes !== r.package_artifact_bytes
          ? el("details", { class: "disclose" }, [
              el("summary", { text: "磁盘实测字节与登记字节不一致（点击查看说明）" }),
              el("div", {}, [
                el("p", {
                  style: "margin:0;font-size:12.5px;color:var(--muted-foreground)",
                  text:
                    `磁盘实测 ${fmtBytes(s.artifact_bytes)}，发布时登记 ${fmtBytes(r.package_artifact_bytes)}。` +
                    " 内容寻址存储按摘要去重，同一份字节被多个版本引用时只落盘一次，" +
                    "因此登记字节通常大于等于磁盘字节；反向的差值才提示发布中断或产物被外部清理。",
                }),
              ]),
            ])
          : null,
      ]),
    );

    // 可运行组合 —— Hub 里唯一「能直接跑」的东西，所以单独提到前面。
    const nav = (hash) => () => go(hash);
    const byKind = r.packages_by_kind || {};
    frag.appendChild(
      el("div", { class: "section" }, [
        el("h4", { class: "section-title", text: "可运行组合" }),
        el("p", {
          style: "margin:-4px 0 10px;font-size:12.5px;color:var(--muted-foreground)",
          text:
            "一个 Episode Stack 把「环境契约 + 基准数据集 + Agent 脚手架 + 运行时网关要求」" +
            "钉成一份可解析的组合。它自己不含字节，只按版本引用下面的构件 —— " +
            "所以同一份数据配不同脚手架，是两个 Stack，而不是两份数据。",
        }),
        el("div", { class: "grid cols-4" }, [
          clickable(
            statTile(
              "Episode Stack",
              fmtNum(r.stacks),
              `${fmtNum(r.stack_versions)} 个版本 · ${fmtNum(r.yanked_stack_versions)} 已撤回`,
            ),
            nav("#/stacks"),
          ),
          clickable(
            statTile("基准数据集", fmtNum(byKind.benchmark ?? 0), "任务目录 + 评测规格"),
            nav("#/benchmarks"),
          ),
          clickable(
            statTile("Agent 脚手架", fmtNum(byKind.agent_scaffold ?? r.agent_bridges), "决定「怎么答」"),
            nav("#/scaffolds"),
          ),
          clickable(
            statTile(
              "环境契约",
              fmtNum(r.active_envs ?? r.envs),
              `${fmtNum(r.env_versions)} 个版本 · ${fmtNum(r.deprecated_envs)} 个历史名已归并`,
            ),
            nav("#/envs"),
          ),
        ]),
      ]),
    );

    // 其余包与运维计数
    const otherKinds = Object.entries(byKind)
      .filter(([k]) => k !== "benchmark" && k !== "agent_scaffold")
      .map(([k, n]) => `${kindMeta(k).label} ${n}`)
      .join(" · ");
    frag.appendChild(
      el("div", { class: "section" }, [
        el("h4", { class: "section-title", text: "存储与运维" }),
        el("div", { class: "grid cols-4" }, [
          clickable(
            statTile(
              "登记产物",
              fmtNum(r.package_artifacts),
              `${fmtNum(r.packages)} 个包 · ${fmtNum(r.package_versions)} 个版本`,
            ),
            nav("#/artifacts"),
          ),
          statTile("其他角色的包", otherKinds || "无", "评分契约 / 镜像包 / 夹具"),
          clickable(statTile("审计条目", fmtNum(r.audit_entries), "发布 / 撤回 / 令牌变更"), nav("#/audit")),
          statTile("有效令牌", fmtNum(r.active_tokens), ov.posture.require_token ? "强制鉴权" : "开放模式"),
        ]),
      ]),
    );

    // 身份与姿态
    frag.appendChild(
      el("div", { class: "grid cols-2 section" }, [
        card(
          "服务身份",
          kv([
            ["名称", ov.service.name],
            ["版本", ov.service.version],
            ["Git SHA", ov.service.git_sha || "未注入（构建时可设 UENV_HUB_GIT_SHA）"],
            ["启动时刻", fmtTime(ov.started_at)],
            ["已运行", fmtDuration(ov.uptime_seconds)],
            ["服务端时间", fmtTime(ov.server_time)],
          ]),
        ),
        card(
          "启动姿态",
          kv([
            [
              "令牌鉴权",
              badge(
                ov.posture.require_token ? "require_token = true" : "require_token = false（开放）",
                ov.posture.require_token ? "ok" : "warn",
              ),
            ],
            [
              "限流",
              ov.posture.rate_limit_enabled
                ? `${ov.posture.requests_per_second} req/s，突发 ${ov.posture.burst}`
                : "已关闭",
            ],
            ["CORS 白名单", ov.posture.cors_allow_origins.join(", ") || "（空）"],
            ["示例包播种", ov.posture.seed_examples ? "开启" : "关闭"],
            ["目录播种源", ov.posture.catalog_seed_dir],
          ]),
        ),
      ]),
    );

    return frag;
  };

  function clickable(node, onClick) {
    node.style.cursor = "pointer";
    node.addEventListener("click", onClick);
    return node;
  }

  // ------------------------------------------------------------ 视图：环境

  routes.envs = async (segs, query) => {
    if (segs.length >= 1) return envDetail(segs[0], segs[1], query);
    setCrumbs("环境契约", "GET /api/v1/envs");

    const page = Number(query.page || 1);
    const perPage = Number(query.per_page || 20);
    const filters = {
      namespace: query.namespace || "",
      author: query.author || "",
      tag: query.tag || "",
    };
    const data = await api(
      `/api/v1/envs${qs({ page, per_page: perPage, ...filters })}`,
    );

    const rebuild = (patch) =>
      go(`#/envs${qs({ page: 1, per_page: perPage, ...filters, ...patch })}`);

    const nsInput = el("input", { type: "text", placeholder: "namespace", value: filters.namespace });
    const authorInput = el("input", { type: "text", placeholder: "author", value: filters.author });
    const tagInput = el("input", { type: "text", placeholder: "tag", value: filters.tag });
    const apply = () =>
      rebuild({
        namespace: nsInput.value.trim(),
        author: authorInput.value.trim(),
        tag: tagInput.value.trim(),
      });
    for (const input of [nsInput, authorInput, tagInput]) {
      input.addEventListener("keydown", (e) => e.key === "Enter" && apply());
    }

    const toolbar = el("div", { class: "toolbar" }, [
      nsInput,
      authorInput,
      tagInput,
      el("button", { class: "btn primary", text: "筛选", onclick: apply }),
      el("button", {
        class: "btn",
        text: "清空",
        onclick: () => go(`#/envs${qs({ per_page: perPage })}`),
      }),
    ]);

    const rows = data.items.map((env) => ({
      onclick: () => go(`#/envs/${encodeURIComponent(env.env_type)}`),
      cells: [
        el("span", {}, [
          el("strong", { text: env.env_type }),
          env.lifecycle && env.lifecycle !== "active"
            ? el("span", { style: "margin-left:8px" }, [
                badge(env.lifecycle, env.lifecycle === "deprecated" ? "warn" : "info"),
              ])
            : null,
        ]),
        env.namespace,
        el("code", { class: "mono", text: env.latest_version || "—" }),
        env.description || "—",
        el("span", {}, env.tags.length ? env.tags.map((t) => badge(t)) : [document.createTextNode("—")]),
        env.author || "—",
        fmtTime(env.updated_at),
      ],
    }));

    // 能力契约与「被当成环境注册的基准」不是一回事：后者已归并到某个契约的
    // dataset 枚举里，只为兼容旧引用而保留可解析。分开列，层级才看得出来。
    const isRetired = (e) => e.lifecycle === "deprecated";
    const live = rows.filter((_, i) => !isRetired(data.items[i]));
    const retired = rows.filter((_, i) => isRetired(data.items[i]));
    const columns = ["环境", "命名空间", "最新版本", "描述", "标签", "作者", "更新时间"];

    const out = el("div", {}, [
      lead(
        "环境契约定义「一次 reset/step 是什么意思、奖励怎么算」，是能力层面的抽象；" +
          "具体考哪些题由基准数据集提供，通过契约的 dataset 路由键挂载。",
      ),
      toolbar,
      card(
        "能力契约",
        el("div", {}, [
          table(columns, live, { empty: "没有匹配的环境契约" }),
          pager(data, (p) => go(`#/envs${qs({ page: p, per_page: perPage, ...filters })}`)),
        ]),
        { tight: true, hint: `${live.length} 个在用契约` },
      ),
    ]);

    if (retired.length) {
      out.appendChild(
        el("div", { class: "section" }, [
          card(
            "已归并的历史名",
            el("div", {}, [
              lead(
                "这些名字曾被注册成独立环境，实际上是某个契约下的一个基准。" +
                  "它们仍以 200 + Deprecation 头可解析，Worker 预热不会因改名而失败；" +
                  "新接入请直接用 superseded_by 指向的契约。",
              ),
              table(columns, retired, { empty: "无" }),
            ]),
            { tight: true, hint: `${retired.length} 个已归并` },
          ),
        ]),
      );
    }
    return out;
  };

  function pager(page, onGo) {
    const totalPages = Math.max(1, Math.ceil(page.total / page.per_page));
    return el("div", { class: "pager" }, [
      el("span", { text: `第 ${page.page} / ${totalPages} 页 · 共 ${page.total} 项` }),
      el("span", { class: "spacer" }),
      el("button", {
        class: "btn sm",
        text: "上一页",
        disabled: page.page <= 1,
        onclick: () => onGo(page.page - 1),
      }),
      el("button", {
        class: "btn sm",
        text: "下一页",
        disabled: page.page >= totalPages,
        onclick: () => onGo(page.page + 1),
      }),
    ]);
  }

  async function envDetail(envType, version, query) {
    setCrumbs(`环境 · ${envType}`, version ? `@${version}` : "");
    const [detail, versions] = await Promise.all([
      api(`/api/v1/envs/${encodeURIComponent(envType)}`),
      api(`/api/v1/envs/${encodeURIComponent(envType)}/versions`),
    ]);

    const selected = version || detail.latest_version || versions[0]?.version;
    const manifest = selected
      ? await api(
          `/api/v1/envs/${encodeURIComponent(envType)}/versions/${encodeURIComponent(selected)}`,
        )
      : null;

    const frag = document.createDocumentFragment();
    frag.appendChild(link("← 返回环境列表", "#/envs", "backlink"));
    frag.appendChild(
      el("div", { class: "title-row" }, [
        el("h2", { text: envType }),
        badge(detail.namespace, "info"),
        detail.lifecycle && detail.lifecycle !== "active"
          ? badge(detail.lifecycle, detail.lifecycle === "deprecated" ? "warn" : "info")
          : null,
        detail.superseded_by ? badge(`→ ${detail.superseded_by}`, "warn") : null,
        el("p", { text: detail.description || "（无描述）" }),
      ]),
    );

    frag.appendChild(
      el("div", { class: "grid cols-2 section" }, [
        card(
          "环境元数据",
          kv([
            ["最新版本", detail.latest_version || "—"],
            ["作者", detail.author || "—"],
            ["许可", detail.license || "—"],
            ["主页", detail.homepage || "—"],
            ["仓库", detail.repository || "—"],
            [
              "标签",
              el("span", {}, detail.tags.length ? detail.tags.map((t) => badge(t)) : [document.createTextNode("—")]),
            ],
            ["兼容别名", (detail.compat_aliases || []).join(", ") || "—"],
            ["创建 / 更新", `${fmtTime(detail.created_at)} / ${fmtTime(detail.updated_at)}`],
          ]),
        ),
        card(
          "版本",
          table(
            ["版本", "状态", "发布时间", "变更"],
            versions.map((v) => ({
              onclick: () =>
                go(`#/envs/${encodeURIComponent(envType)}/${encodeURIComponent(v.version)}`),
              cells: [
                el("code", { class: "mono", text: v.version }),
                v.is_yanked ? badge("已撤回", "bad") : badge("可用", "ok"),
                fmtTime(v.published_at),
                v.changelog || "—",
              ],
            })),
            { empty: "尚未发布任何版本" },
          ),
          { tight: true, hint: `${versions.length} 个版本` },
        ),
      ]),
    );

    if (manifest) frag.appendChild(manifestCard(envType, manifest));
    frag.appendChild(resolveCard(envType, query));
    return frag;
  }

  function manifestCard(envType, m) {
    const res = m.resources || {};
    const img = m.image || {};
    const body = el("div", {}, [
      kv([
        ["版本", el("code", { class: "mono", text: m.version })],
        ["状态", m.is_yanked ? badge(`已撤回：${m.yank_reason || "未说明"}`, "bad") : badge("可用", "ok")],
        ["入口", m.entrypoint || "—"],
        ["支持后端", (m.supported_backends || []).join(", ") || "—"],
        ["基础镜像", m.base_image || "—"],
        ["健康检查", m.health_check_path || "—"],
        ["最低 UEnv 版本", m.min_uenv_version || "—"],
        ["镜像引用", img.url || "—"],
        ["镜像摘要", el("span", { class: "digest", text: img.digest || "—" })],
        [
          "镜像大小 / 架构",
          `${img.size_bytes ? fmtBytes(img.size_bytes) : "—"} · ${img.arch || "—"}`,
        ],
        [
          "资源诉求",
          `CPU ${res.cpu ?? "—"} · 内存 ${res.memory_mb ? `${res.memory_mb} MiB` : "—"} · GPU ${res.gpu ?? 0}${res.gpu_type ? ` (${res.gpu_type})` : ""}`,
        ],
        ["发布时间", fmtTime(m.published_at)],
      ]),
    ]);

    if (m.deprecation) {
      body.prepend(
        el("div", {
          class: "err",
          text: `弃用提示：${m.deprecation.message}${m.deprecation.superseded_by ? `（继任者 ${m.deprecation.superseded_by}）` : ""}`,
        }),
      );
    }
    if ((m.gate_notes || []).length) {
      body.appendChild(disclose(`发布闸门备注（${m.gate_notes.length}）`, jsonBlock(m.gate_notes)));
    }
    if (m.rubric) body.appendChild(disclose("Rubric 判分契约", jsonBlock(m.rubric)));
    if (m.config_schema) body.appendChild(disclose("配置 Schema", jsonBlock(m.config_schema)));
    if (m.default_config) body.appendChild(disclose("默认配置", jsonBlock(m.default_config)));
    if (m.interface) body.appendChild(disclose("接口契约 action / observation / state", jsonBlock(m.interface)));
    if ((m.examples || []).length) body.appendChild(disclose(`示例（${m.examples.length}）`, jsonBlock(m.examples)));
    if (m.dependencies) body.appendChild(disclose("依赖", jsonBlock(m.dependencies)));
    body.appendChild(disclose("完整 Manifest JSON", jsonBlock(m)));

    return el("div", { class: "section" }, [
      card(`版本清单 ${envType}@${m.version}`, body, {
        hint: `GET /api/v1/envs/${envType}/versions/${m.version}`,
      }),
    ]);
  }

  function resolveCard(envType, query) {
    const input = el("input", {
      type: "text",
      placeholder: "语义化约束，例如 ^1.0 或 >=0.2, <0.4",
      value: query.constraint || "",
      style: "flex:1;min-width:220px",
    });
    const out = el("div", { style: "margin-top:12px" });
    const run = async () => {
      const constraint = input.value.trim();
      if (!constraint) {
        out.replaceChildren(el("div", { class: "empty", text: "请输入约束" }));
        return;
      }
      out.replaceChildren(el("div", { class: "empty", text: "解析中…" }));
      try {
        const m = await api(
          `/api/v1/envs/${encodeURIComponent(envType)}/resolve${qs({ constraint })}`,
        );
        out.replaceChildren(
          el("div", {}, [
            el("p", { style: "margin:0 0 8px" }, [
              document.createTextNode("解析结果："),
              badge(`${m.env_type}@${m.version}`, "ok"),
            ]),
            jsonBlock(m),
          ]),
        );
      } catch (error) {
        out.replaceChildren(renderError(error));
      }
    };
    input.addEventListener("keydown", (e) => e.key === "Enter" && run());

    return el("div", { class: "section" }, [
      card(
        "版本约束解析",
        el("div", {}, [
          el("div", { class: "toolbar", style: "margin-bottom:0" }, [
            input,
            el("button", { class: "btn primary", text: "解析", onclick: run }),
          ]),
          el("p", {
            style: "margin:8px 0 0;font-size:12.5px;color:var(--muted-foreground)",
            text: "解析只在未撤回的版本中选取满足约束的最高版本，与 Worker 启动时走的是同一条服务端逻辑。",
          }),
          out,
        ]),
        { hint: `GET /api/v1/envs/${envType}/resolve` },
      ),
    ]);
  }

  // ---------------------------------------------------------- 视图：环境包

  const lead = (text) =>
    el("p", { style: "margin:0 0 14px;color:var(--muted-foreground);font-size:13px", text });

  /** `#/packages` 只保留详情路由；列表按角色拆到了基准 / 脚手架 / 制品三页。 */
  routes.packages = async (segs) => {
    if (segs.length >= 1) return packageDetail(segs[0], segs[1]);
    go("#/benchmarks");
    return el("div", { class: "empty", text: "正在跳转到「基准与数据集」…" });
  };

  const pkgRow = (p, extra) => ({
    onclick: () => go(`#/packages/${encodeURIComponent(p.package_id)}`),
    cells: [
      el("strong", { text: p.package_id }),
      el("code", { class: "mono", text: p.latest_version || "—" }),
      ...(extra ? extra(p) : []),
      p.description || "—",
      fmtTime(p.updated_at),
    ],
  });

  // ------------------------------------------------------ 视图：基准与数据集
  //
  // 层级：环境契约（交互怎么定义）→ 基准/数据集（考哪些题）→ 制品（字节）。
  // 三个正式契约固定展示，避免「未声明」把本该归类的包甩到一边。

  const ENV_CONTRACTS = [
    {
      id: "swe",
      title: "SWE · 仓库级缺陷修复",
      blurb:
        "容器内多轮修 bug。verified / pro / smith 是同一契约下的三个数据集变体，" +
        "差别在题目集、镜像命名与 grader，不是三种环境。",
    },
    {
      id: "qa",
      title: "QA · 单轮问答 / 判分",
      blurb:
        "一道题一次作答，按标准答案或 rubric 判分。olymmath / pubmedqa / scitab 等" +
        "都是挂在 qa 下的数据集（历史名 math 已归并到 qa）。",
    },
    {
      id: "code",
      title: "Code · 代码执行",
      blurb: "生成代码并跑测试拿奖励。当前挂载的数据集是 DSCodeBench。",
    },
  ];

  const datasetLabel = (p) => {
    if (p.dataset) return p.dataset;
    const id = p.package_id || "";
    if (id.startsWith("swe-bench-")) return id.slice("swe-bench-".length);
    return "—";
  };

  routes.benchmarks = async () => {
    setCrumbs("基准与数据集", "环境契约 → 数据集变体");
    const { groups } = await packagesByKind();
    // 正式训练数据与 smoke fixture 分开：后者不是「又一种契约」。
    const benches = groups.benchmark || [];
    const fixtures = groups.fixture || [];

    const byEnv = {};
    for (const b of benches) {
      const k = b.env_type || "_unknown";
      (byEnv[k] = byEnv[k] || []).push(b);
    }
    for (const list of Object.values(byEnv)) {
      list.sort((a, b) => a.package_id.localeCompare(b.package_id));
    }

    const frag = document.createDocumentFragment();
    frag.appendChild(
      el("div", { class: "model-card" }, [
        el("h3", { text: "怎么读这一页" }),
        el("ol", { class: "model-steps" }, [
          el("li", {
            html: "<strong>环境契约</strong>（swe / qa / code）定义「一次 reset/step 是什么、奖励怎么算」。",
          }),
          el("li", {
            html: "<strong>基准数据集</strong>是契约下的题目与镜像打包，发给 Worker 的内容寻址单元。",
          }),
          el("li", {
            html: "<strong>变体 / dataset</strong>是契约内的路由键，例如 swe 下的 <code>smith</code>，不是新的环境类型。",
          }),
        ]),
        el("p", {
          class: "model-note",
          text: "Episode Stack 再往上选一层：把「契约 + 某个数据集 + Agent 脚手架」钉成可运行组合。",
        }),
      ]),
    );

    for (const contract of ENV_CONTRACTS) {
      const list = byEnv[contract.id] || [];
      delete byEnv[contract.id];
      const totalInstances = list.reduce((a, x) => a + (x.instance_count || 0), 0);
      const body = el("div", {}, [
        el("p", { class: "contract-blurb", text: contract.blurb }),
        list.length
          ? table(
              [
                "基准包",
                "变体 / dataset",
                "最新版本",
                { label: "实例数", num: true },
                "描述",
                "更新时间",
              ],
              list.map((p) => ({
                onclick: () => go(`#/packages/${encodeURIComponent(p.package_id)}`),
                cells: [
                  el("strong", { text: p.package_id }),
                  badge(datasetLabel(p), "info"),
                  el("code", { class: "mono", text: p.latest_version || "—" }),
                  p.instance_count ? fmtNum(p.instance_count) : "—",
                  p.description || "—",
                  fmtTime(p.updated_at),
                ],
              })),
              { empty: "该契约下尚未挂载基准" },
            )
          : el("div", { class: "empty", text: "该契约下尚未挂载基准数据集" }),
      ]);
      frag.appendChild(
        el("div", { class: "section" }, [
          card(contract.title, body, {
            tight: true,
            hint:
              `${list.length} 个数据集` +
              (totalInstances ? ` · 合计 ${fmtNum(totalInstances)} 条实例` : ""),
            actions: [
              el("a", {
                class: "btn sm",
                href: `#/envs/${encodeURIComponent(contract.id)}`,
                text: `契约 ${contract.id}`,
              }),
            ],
          }),
        ]),
      );
    }

    const leftovers = Object.entries(byEnv).flatMap(([, list]) => list);
    if (leftovers.length) {
      frag.appendChild(
        el("div", { class: "section" }, [
          card(
            "尚无法归入上述契约",
            el("div", {}, [
              el("p", {
                class: "contract-blurb",
                text:
                  "这些包有 catalog，但 overlay / 包名都无法映射到 swe、qa、code。" +
                  "需要补 worker_overlay.env_type 或按命名规范发布。",
              }),
              table(
                ["基准包", "最新版本", "描述", "更新时间"],
                leftovers.map((p) => pkgRow(p)),
                { empty: "无" },
              ),
            ]),
            { tight: true, hint: `${leftovers.length} 个` },
          ),
        ]),
      );
    }

    if (fixtures.length) {
      frag.appendChild(
        el("div", { class: "section" }, [
          card(
            "联调夹具（非训练基准）",
            el("div", {}, [
              el("p", {
                class: "contract-blurb",
                text: "smoke / fixture 包只用于预热与回归，不计入正式基准目录。",
              }),
              table(
                ["包 ID", "归属契约", "最新版本", "描述", "更新时间"],
                fixtures.map((p) => ({
                  onclick: () => go(`#/packages/${encodeURIComponent(p.package_id)}`),
                  cells: [
                    el("strong", { text: p.package_id }),
                    p.env_type ? badge(p.env_type) : "—",
                    el("code", { class: "mono", text: p.latest_version || "—" }),
                    p.description || "—",
                    fmtTime(p.updated_at),
                  ],
                })),
                { empty: "无" },
              ),
            ]),
            { tight: true, hint: `${fixtures.length} 个夹具` },
          ),
        ]),
      );
    }

    return frag;
  };

  // ------------------------------------------------------- 视图：Agent 脚手架

  routes.scaffolds = async () => {
    setCrumbs("Agent 脚手架", "kind=agent_scaffold · GET /api/v1/agent-bridges");
    const [{ groups }, bridges] = await Promise.all([
      packagesByKind(),
      api("/api/v1/agent-bridges").catch(() => []),
    ]);
    const scaffolds = groups.agent_scaffold || [];
    // Agent Bridge 目录不是另一类实体，而是同一批脚手架包按 `agent_kind` 的投影；
    // 这里把两者并到一张表，避免同一个包在导航里被数两遍。
    const byId = new Map((Array.isArray(bridges) ? bridges : []).map((b) => [b.package_id, b]));

    return el("div", {}, [
      lead(
        "脚手架决定「怎么答」：它跑在 Agent 宿主上，通过 Worker Runtime Gateway 把命令" +
          "路由回任务容器。它本身也是环境包，只是在 agent_defaults 里声明了 agent_kind，" +
          "因此同时出现在 Agent Bridge 目录里 —— 是同一个对象的两种视图，不是两样东西。",
      ),
      card(
        "脚手架注册表",
        table(
          ["包 ID", "最新版本", "agent_kind", "可驱动环境", "描述", "更新时间"],
          scaffolds.map((p) =>
            pkgRow(p, (x) => {
              const b = byId.get(x.package_id);
              return [
                b?.agent_kind ? badge(b.agent_kind, "info") : "—",
                (b?.required_env_types || []).join(", ") || "—",
              ];
            }),
          ),
          { empty: "尚未发布 Agent 脚手架" },
        ),
        { tight: true, hint: `${scaffolds.length} 个脚手架` },
      ),
    ]);
  };

  // ------------------------------------------------------- 视图：制品与镜像

  routes.artifacts = async () => {
    setCrumbs("制品与镜像", "各包 manifest 的产物汇总");
    const { items } = await packagesByKind();

    // 逐包取最新版本清单，把产物摊平成一张「字节从哪来」的表。包数是两位数量级，
    // 顺序请求即可；真正大的是产物本身，而产物字节从不进这个页面。
    const rows = [];
    let totalBytes = 0;
    let tarBytes = 0;
    for (const p of items) {
      if (!p.latest_version) continue;
      let m;
      try {
        m = await api(
          `/api/v1/packages/${encodeURIComponent(p.package_id)}/versions/${encodeURIComponent(p.latest_version)}`,
        );
      } catch {
        continue;
      }
      for (const a of m.artifacts || []) {
        const size = a.size_bytes || 0;
        totalBytes += size;
        const isTar = a.name.endsWith(".tar") || a.kind === "image_tar";
        if (isTar) tarBytes += size;
        rows.push({
          size,
          onclick: () => go(`#/packages/${encodeURIComponent(p.package_id)}`),
          cells: [
            el("code", { class: "mono", text: a.name }),
            isTar ? badge("镜像 tar", "info") : a.kind || "—",
            el("span", {}, [link(p.package_id, `#/packages/${encodeURIComponent(p.package_id)}`)]),
            el("code", { class: "mono", text: p.latest_version }),
            size ? fmtBytes(size) : "—",
            a.sync_mode || "—",
            el("span", { class: "digest", text: shortDigest(a.digest), title: a.digest }),
          ],
        });
      }
    }
    rows.sort((a, b) => b.size - a.size);

    return el("div", {}, [
      lead(
        "制品是内容寻址的字节：Hub 按 sha256 存一份，消费侧按摘要取。镜像 tar 由 Hub 托管时，" +
          "Worker 可以 docker load 而无需外部 registry —— 这就是零外拉。",
      ),
      el("div", { class: "grid cols-3 section" }, [
        statTile("产物总数", fmtNum(rows.length), "全部包的最新版本"),
        statTile("登记字节", fmtBytes(totalBytes), "发布时记录的大小"),
        statTile("其中镜像 tar", fmtBytes(tarBytes), "可供零外拉 docker load"),
      ]),
      card(
        "产物清单",
        table(
          ["名称", "类型", "所属包", "版本", { label: "大小", num: true }, "同步方式", "摘要"],
          rows,
          { empty: "尚无登记产物" },
        ),
        { tight: true, hint: "按大小降序" },
      ),
    ]);
  };

  async function packageDetail(packageId, version) {
    const v = version || "latest";
    setCrumbs(`环境包 · ${packageId}`, `@${v}`);
    const manifest = await api(
      `/api/v1/packages/${encodeURIComponent(packageId)}/versions/${encodeURIComponent(v)}`,
    );
    const plan = await api(
      `/api/v1/packages/${encodeURIComponent(packageId)}/versions/${encodeURIComponent(manifest.version)}/sync-plan`,
    );
    // 描述挂在注册表条目上而不是版本清单上，详情页要回到列表里取。
    // 列表端点只支持分页（无按 id 过滤），故一次拉满一页再匹配。
    const summary = await api(`/api/v1/packages${qs({ per_page: 200 })}`).catch(() => null);
    const entry = (summary?.items || []).find((p) => p.package_id === packageId);

    const meta = kindMeta(entry?.kind);
    const backTo =
      entry?.kind === "agent_scaffold"
        ? ["← 返回 Agent 脚手架", "#/scaffolds"]
        : entry?.kind === "benchmark"
          ? ["← 返回基准与数据集", "#/benchmarks"]
          : ["← 返回制品与镜像", "#/artifacts"];

    const frag = document.createDocumentFragment();
    frag.appendChild(link(backTo[0], backTo[1], "backlink"));
    frag.appendChild(
      el("div", { class: "title-row" }, [
        el("h2", { text: packageId }),
        badge(manifest.version, "info"),
        badge(meta.label, meta.badge),
        el("p", { text: entry?.description || manifest.description || "（无描述）" }),
      ]),
    );

    // SWE 基准的实例明细自己就是一份大目录，放进详情页而不是顶级导航：
    // 它是这个包的内容，不是与包平级的概念。
    const sweVariant = manifest.worker_overlay?.swe?.benchmark_variant;
    if (sweVariant) {
      frag.appendChild(
        el("div", { class: "section" }, [
          el("a", {
            class: "btn",
            href: `#/swe${qs({ variant: sweVariant })}`,
            text: `浏览该基准的实例目录（variant=${sweVariant}）`,
          }),
        ]),
      );
    }

    const artifactBytes = (manifest.artifacts || []).reduce((a, x) => a + (x.size_bytes || 0), 0);
    frag.appendChild(
      el("div", { class: "grid cols-4 section" }, [
        statTile("产物数", fmtNum((manifest.artifacts || []).length), "内容寻址"),
        statTile("产物字节", fmtBytes(artifactBytes), "发布时登记"),
        statTile("Bundle 摘要", shortDigest(plan.bundle_digest).replace("sha256:", ""), "sync 的 .synced 标记值"),
        statTile(
          "Worker 平台要求",
          manifest.platform?.uenv_worker_min || "—",
          (manifest.platform?.features || []).join(", ") || "无附加特性",
        ),
      ]),
    );

    frag.appendChild(
      el("div", { class: "section" }, [
        card(
          "产物清单",
          table(
            ["名称", "类型", "摘要", { label: "大小", num: true }, "同步方式", "落地路径", "操作"],
            (manifest.artifacts || []).map((a) => ({
              cells: [
                el("code", { class: "mono", text: a.name }),
                a.kind || "—",
                el("span", { class: "digest", text: shortDigest(a.digest), title: a.digest }),
                a.size_bytes ? fmtBytes(a.size_bytes) : "—",
                a.sync_mode || "—",
                el("code", { class: "mono", text: a.target_rel_path || "—" }),
                el("button", {
                  class: "btn sm",
                  text: "查看",
                  onclick: () => openArtifact(packageId, manifest.version, a.name),
                }),
              ],
            })),
            { empty: "该版本没有登记产物" },
          ),
          {
            tight: true,
            hint: `GET /api/v1/packages/${packageId}/versions/${manifest.version}/artifacts/{name}`,
          },
        ),
      ]),
    );

    const details = el("div", {});
    details.appendChild(
      kv([
        ["包 ID", manifest.package_id],
        ["版本", el("code", { class: "mono", text: manifest.version })],
        ["发布者", manifest.publisher || "—"],
        ["发布时间", fmtTime(manifest.published_at)],
        ["Bundle 摘要", el("span", { class: "digest", text: plan.bundle_digest })],
      ]),
    );
    if (manifest.worker_overlay) details.appendChild(disclose("Worker Overlay", jsonBlock(manifest.worker_overlay), true));
    if (manifest.agent_defaults) details.appendChild(disclose("Agent 默认值", jsonBlock(manifest.agent_defaults)));
    if (manifest.contracts) details.appendChild(disclose("契约声明", jsonBlock(manifest.contracts)));
    if (manifest.interface) details.appendChild(disclose("接口契约", jsonBlock(manifest.interface)));
    details.appendChild(disclose("同步计划 sync-plan", jsonBlock(plan)));
    details.appendChild(disclose("完整包清单 JSON", jsonBlock(manifest)));

    frag.appendChild(el("div", { class: "section" }, [card("包定义", details)]));
    return frag;
  }

  async function openArtifact(packageId, version, name) {
    const url = `/api/v1/packages/${encodeURIComponent(packageId)}/versions/${encodeURIComponent(version)}/artifacts/${encodeURIComponent(name)}`;
    try {
      const headers = store.token ? { Authorization: `Bearer ${store.token}` } : {};
      const resp = await fetch(url, { headers });
      if (!resp.ok) throw new Error(`${resp.status} ${resp.statusText}`);
      const digest = resp.headers.get("etag");
      const text = await resp.text();
      let rendered;
      try {
        rendered = jsonBlock(JSON.parse(text));
      } catch {
        rendered = el("pre", { class: "json", text });
      }
      const host = el("div", { class: "section" }, [
        card(`产物 ${name}`, el("div", {}, [
          kv([
            ["大小", fmtBytes(new Blob([text]).size)],
            ["ETag（存储摘要）", el("span", { class: "digest", text: digest || "—" })],
            ["下载", el("a", { class: "btn sm", href: url, download: name, text: "另存为" })],
          ]),
          el("div", { style: "margin-top:10px" }, [rendered]),
        ])),
      ]);
      $("#view").appendChild(host);
      host.scrollIntoView({ behavior: "smooth", block: "start" });
    } catch (error) {
      toast(`读取产物失败：${error.message}`, "bad");
    }
  }

  // ------------------------------------------------------ 视图：Episode Stack

  routes.stacks = async (segs, query) => {
    if (segs.length >= 1) return stackDetail(segs[0], segs[1]);
    setCrumbs("Episode Stack", "GET /api/v1/episode-stacks");

    const page = Number(query.page || 1);
    const perPage = Number(query.per_page || 20);
    const data = await api(`/api/v1/episode-stacks${qs({ page, per_page: perPage })}`);

    const rows = data.items.map((s) => ({
      onclick: () => go(`#/stacks/${encodeURIComponent(s.stack_id)}`),
      cells: [
        el("strong", { text: s.stack_id }),
        el("code", { class: "mono", text: s.latest_version || "—" }),
        badge(s.execution_mode, s.execution_mode === "agent" ? "info" : ""),
        el("code", { class: "mono", text: s.task_env_type }),
        s.agent_package_id ? el("code", { class: "mono", text: s.agent_package_id }) : "—",
        s.gateway_required ? badge("必需", "warn") : badge("不需要", "ok"),
        s.description || "—",
      ],
    }));

    return el("div", {}, [
      el("p", {
        style: "margin:0 0 14px;color:var(--muted-foreground);font-size:13px",
        text:
          "Episode Stack 把「任务环境 + Agent 脚手架 + 运行时网关」登记为一个可解析的整体，" +
          "解析时把所有浮动约束钉到具体版本，并给出一个可写进训练记录的 stack_digest。",
      }),
      card(
        "Episode Stack 注册表",
        el("div", {}, [
          table(
            ["Stack ID", "最新版本", "执行模式", "任务环境", "Agent 脚手架", "运行时网关", "描述"],
            rows,
            { empty: "尚未发布任何 Episode Stack" },
          ),
          pager(data, (p) => go(`#/stacks${qs({ page: p, per_page: perPage })}`)),
        ]),
        { tight: true, hint: `共 ${data.total} 项` },
      ),
    ]);
  };

  async function stackDetail(stackId, version) {
    setCrumbs(`Episode Stack · ${stackId}`, version ? `@${version}` : "");
    const versions = await api(
      `/api/v1/episode-stacks/${encodeURIComponent(stackId)}/versions`,
    );
    const selected = version || versions.find((v) => !v.is_yanked)?.version || versions[0]?.version;
    if (!selected) {
      return el("div", { class: "empty", text: "该 Stack 尚未发布任何版本" });
    }

    const [manifest, resolved] = await Promise.all([
      api(
        `/api/v1/episode-stacks/${encodeURIComponent(stackId)}/versions/${encodeURIComponent(selected)}`,
      ),
      api(
        `/api/v1/episode-stacks/${encodeURIComponent(stackId)}/versions/${encodeURIComponent(selected)}/resolve`,
      ).catch((e) => ({ __error: e })),
    ]);

    const frag = document.createDocumentFragment();
    frag.appendChild(link("← 返回 Stack 列表", "#/stacks", "backlink"));
    frag.appendChild(
      el("div", { class: "title-row" }, [
        el("h2", { text: stackId }),
        badge(manifest.version, "info"),
        badge(manifest.execution_mode, "info"),
        el("p", { text: manifest.description || "（无描述）" }),
      ]),
    );

    frag.appendChild(
      el("div", { class: "grid cols-2 section" }, [
        card(
          "版本",
          table(
            ["版本", "状态", "发布时间"],
            versions.map((v) => ({
              onclick: () =>
                go(`#/stacks/${encodeURIComponent(stackId)}/${encodeURIComponent(v.version)}`),
              cells: [
                el("code", { class: "mono", text: v.version }),
                v.is_yanked ? badge("已撤回", "bad") : badge("可用", "ok"),
                fmtTime(v.published_at),
              ],
            })),
          ),
          { tight: true },
        ),
        card(
          "声明",
          kv([
            ["任务环境", `${manifest.task_env?.env_type || "—"}@${manifest.task_env?.version || "—"}`],
            ["数据集", manifest.task_env?.dataset || "—"],
            ["Agent 脚手架", scaffoldRef(manifest.agent_scaffold)],
            ["环境包", (manifest.env_packages || []).join(", ") || "—"],
            [
              "运行时网关",
              manifest.runtime_gateway?.required ? badge("必需", "warn") : badge("不需要", "ok"),
            ],
            [
              "所需 Worker 能力",
              (manifest.required_worker_features || []).join(", ") || "—",
            ],
            ["执行模式", manifest.execution_mode || "—"],
            ["发布时间", fmtTime(manifest.published_at)],
          ]),
        ),
      ]),
    );

    if (resolved.__error) {
      frag.appendChild(el("div", { class: "section" }, [renderError(resolved.__error)]));
    } else {
      const comps = table(
        ["角色", "标识", "请求约束", "解析版本", "摘要", "来源"],
        (resolved.components || []).map((c) => ({
          cells: [
            badge(c.role, c.role === "task_env" ? "info" : ""),
            el("strong", { text: c.id }),
            el("code", { class: "mono", text: c.requested }),
            el("code", { class: "mono", text: c.resolved }),
            el("span", { class: "digest", text: shortDigest(c.digest), title: c.digest || "" }),
            c.url ? el("code", { class: "mono", text: c.url }) : "—",
          ],
        })),
        { empty: "解析结果没有组件" },
      );

      const body = el("div", {}, [
        el("div", { class: "grid cols-2", style: "margin-bottom:12px" }, [
          statTile("Stack 摘要", shortDigest(resolved.stack_digest).replace("sha256:", ""), "可写入训练记录以标识整栈"),
          statTile(
            "运行时网关",
            resolved.runtime_gateway?.required ? "必需" : "不需要",
            resolved.runtime_gateway?.protocol || "",
          ),
        ]),
        comps,
      ]);
      if ((resolved.notes || []).length) {
        body.appendChild(disclose(`解析备注（${resolved.notes.length}）`, jsonBlock(resolved.notes), true));
      }
      if ((resolved.package_plans || []).length) {
        body.appendChild(disclose(`环境包同步计划（${resolved.package_plans.length}）`, jsonBlock(resolved.package_plans)));
      }
      if (resolved.agent_scaffold) body.appendChild(disclose("Agent 脚手架", jsonBlock(resolved.agent_scaffold)));
      body.appendChild(disclose("任务环境清单", jsonBlock(resolved.task_env_manifest)));
      body.appendChild(disclose("完整解析结果 JSON", jsonBlock(resolved)));

      frag.appendChild(
        el("div", { class: "section" }, [
          card("解析后的启动计划", body, {
            hint: `GET /api/v1/episode-stacks/${stackId}/versions/${selected}/resolve`,
          }),
        ]),
      );
    }

    frag.appendChild(
      el("div", { class: "section" }, [card("Stack 声明 JSON", jsonBlock(manifest))]),
    );
    return frag;
  }

  // ------------------------------------------------------ 视图：Agent Bridge

  routes.bridges = async () => {
    setCrumbs("Agent Bridge", "GET /api/v1/agent-bridges");
    const items = await api("/api/v1/agent-bridges");
    return el("div", {}, [
      el("p", {
        style: "margin:0 0 14px;color:var(--muted-foreground);font-size:13px",
        text:
          "这是「Agent 脚手架」按 agent_kind 的投影视图，内容与该页同源；" +
          "bundle_digest 与 Agent 注册时上报的字段同名同值，两侧可以直接比对。",
      }),
      card(
        "Agent Bridge 投影",
        table(
          ["包 ID", "版本", "脚手架族", "驱动的环境", "Worker 特性要求", "Bundle 摘要", "发布时间"],
          items.map((b) => ({
            onclick: () => go(`#/packages/${encodeURIComponent(b.package_id)}/${encodeURIComponent(b.version)}`),
            cells: [
              el("strong", { text: b.package_id }),
              el("code", { class: "mono", text: b.version }),
              b.agent_kind ? badge(b.agent_kind, "info") : "—",
              el("span", {}, (b.required_env_types || []).map((t) => badge(t))),
              el("span", {}, (b.required_worker_features || []).map((t) => badge(t))),
              el("span", { class: "digest", text: shortDigest(b.bundle_digest), title: b.bundle_digest }),
              fmtTime(b.published_at),
            ],
          })),
          { empty: "没有已发布的 Agent 脚手架" },
        ),
        { tight: true, hint: `${items.length} 个脚手架` },
      ),
    ]);
  };

  // ------------------------------------------------------- 视图：SWE 实例目录

  const SWE_VARIANTS = ["verified", "lite", "pro", "smith"];

  routes.swe = async (segs, query) => {
    const variant = query.variant || segs[0] || "verified";
    setCrumbs("SWE 实例目录", `GET /api/v1/swe/${variant}/instances`);

    const tabs = el(
      "div",
      { class: "tabs" },
      SWE_VARIANTS.map((v) =>
        el("button", {
          class: v === variant ? "tab active" : "tab",
          text: v,
          onclick: () => go(`#/swe${qs({ variant: v })}`),
        }),
      ),
    );

    let body;
    try {
      const catalog = await api(`/api/v1/swe/${encodeURIComponent(variant)}/instances`);
      // 目录有三种在用形态：数组、{instances:[...]}，以及 config/swe/*.json
      // 实际采用的「以 instance_id 为键的字典」。
      const list = Array.isArray(catalog)
        ? catalog
        : Array.isArray(catalog?.instances)
          ? catalog.instances
          : catalog && typeof catalog === "object"
            ? Object.values(catalog).filter((v) => v && typeof v === "object")
            : [];
      const rows = list.slice(0, 200).map((inst) => ({
        cells: [
          el("code", { class: "mono", text: inst.instance_id || inst.id || "—" }),
          inst.repo || "—",
          el("code", { class: "mono", text: (inst.base_commit || "").slice(0, 12) || "—" }),
          el("code", { class: "mono", text: inst.environment_setup_commit ? String(inst.environment_setup_commit).slice(0, 12) : "—" }),
          inst.version || "—",
        ],
      }));
      body = el("div", {}, [
        el("div", { class: "grid cols-4", style: "margin-bottom:14px" }, [
          statTile("实例总数", fmtNum(list.length), `变体 ${variant}`),
          statTile(
            "涉及仓库",
            fmtNum(new Set(list.map((i) => i.repo).filter(Boolean)).size),
            "去重后的 repo 数",
          ),
          statTile(
            "带镜像声明",
            fmtNum(list.filter((i) => i.image || i.docker_image).length),
            "catalog 内直接给出镜像的实例",
          ),
          statTile("展示上限", "200", "超出部分请用 API 拉取"),
        ]),
        card(
          `实例列表 · ${variant}`,
          table(["实例 ID", "仓库", "base_commit", "env_setup_commit", "版本"], rows, {
            empty: "该变体目录为空",
          }),
          { tight: true, hint: list.length > 200 ? `仅显示前 200 / 共 ${list.length}` : `共 ${list.length}` },
        ),
      ]);
    } catch (error) {
      body = renderError(error);
    }

    return el("div", {}, [
      el("p", {
        style: "margin:0 0 12px;color:var(--muted-foreground);font-size:13px",
        text: "实例目录由 Hub 从 UENV_HUB_SWE_CATALOG_DIR 指向的目录读取并校验 JSON 后原样下发，Worker 据此拿到实例真值。",
      }),
      tabs,
      body,
    ]);
  };

  // -------------------------------------------------------- 视图：模板

  routes.templates = async () => {
    setCrumbs("脚手架模板", "GET /api/v1/templates");
    const items = await api("/api/v1/templates");
    return card(
      "环境脚手架模板",
      table(
        ["名称", "版本", "描述", "归档 SHA256", "更新时间", "操作"],
        items.map((t) => ({
          cells: [
            el("strong", { text: t.name }),
            el("code", { class: "mono", text: t.version }),
            t.description || "—",
            el("span", { class: "digest", text: shortDigest(t.archive_sha256), title: t.archive_sha256 || "" }),
            fmtTime(t.updated_at),
            el("a", {
              class: "btn sm",
              href: `/api/v1/templates/${encodeURIComponent(t.name)}/archive`,
              download: `${t.name}.tar.gz`,
              text: "下载归档",
            }),
          ],
        })),
        { empty: "没有已播种的模板" },
      ),
      { tight: true, hint: `uenv env init <name> --template <模板>` },
    );
  };

  // -------------------------------------------------------- 视图：搜索

  routes.search = async (segs, query) => {
    setCrumbs("搜索", "GET /api/v1/search");
    const q = query.q || "";
    const tag = query.tag || "";
    const author = query.author || "";
    const namespace = query.namespace || "";

    const qi = el("input", { type: "text", placeholder: "关键词（匹配环境名与描述）", value: q, class: "grow" });
    const ti = el("input", { type: "text", placeholder: "tag", value: tag });
    const ai = el("input", { type: "text", placeholder: "author", value: author });
    const ni = el("input", { type: "text", placeholder: "namespace", value: namespace });
    const run = () =>
      go(
        `#/search${qs({ q: qi.value.trim(), tag: ti.value.trim(), author: ai.value.trim(), namespace: ni.value.trim() })}`,
      );
    for (const input of [qi, ti, ai, ni]) {
      input.addEventListener("keydown", (e) => e.key === "Enter" && run());
    }

    const toolbar = el("div", { class: "toolbar" }, [
      qi,
      ti,
      ai,
      ni,
      el("button", { class: "btn primary", text: "搜索", onclick: run }),
    ]);

    if (!q && !tag && !author && !namespace) {
      return el("div", {}, [toolbar, el("div", { class: "empty", text: "输入条件后开始搜索" })]);
    }

    const data = await api(`/api/v1/search${qs({ q, tag, author, namespace })}`);
    const rows = data.results.map((env) => ({
      onclick: () => go(`#/envs/${encodeURIComponent(env.env_type)}`),
      cells: [
        el("strong", { text: env.env_type }),
        env.namespace,
        el("code", { class: "mono", text: env.latest_version || "—" }),
        env.description || "—",
        el("span", {}, env.tags.map((t) => badge(t))),
      ],
    }));

    return el("div", {}, [
      toolbar,
      card(
        "搜索结果",
        table(["环境", "命名空间", "最新版本", "描述", "标签"], rows, { empty: "没有匹配项" }),
        { tight: true, hint: `命中 ${data.total} 项` },
      ),
    ]);
  };

  // ------------------------------------------------------ 视图：审计日志

  routes.audit = async (segs, query) => {
    setCrumbs("审计日志", "GET /api/v1/admin/audit-log（需要 admin）");
    const page = Number(query.page || 1);
    const perPage = Number(query.per_page || 50);
    const items = await api(`/api/v1/admin/audit-log${qs({ page, per_page: perPage })}`);

    const rows = items.map((e) => ({
      cells: [
        String(e.id),
        fmtTime(e.timestamp),
        e.actor || "—",
        badge(e.action, e.action === "DELETE" || e.action === "YANK" ? "warn" : "ok"),
        e.resource_type,
        el("code", { class: "mono", text: e.resource_id || "—" }),
        e.source_ip || "—",
      ],
    }));

    return el("div", {}, [
      card(
        "审计条目",
        el("div", {}, [
          table(["ID", "时间", "操作者", "动作", "资源类型", "资源", "来源 IP"], rows, {
            empty: "本页没有审计记录",
          }),
          el("div", { class: "pager" }, [
            el("span", { text: `第 ${page} 页 · 本页 ${items.length} 条` }),
            el("span", { class: "spacer" }),
            el("button", {
              class: "btn sm",
              text: "上一页",
              disabled: page <= 1,
              onclick: () => go(`#/audit${qs({ page: page - 1, per_page: perPage })}`),
            }),
            el("button", {
              class: "btn sm",
              text: "下一页",
              disabled: items.length < perPage,
              onclick: () => go(`#/audit${qs({ page: page + 1, per_page: perPage })}`),
            }),
          ]),
        ]),
        { tight: true, hint: "按时间倒序" },
      ),
    ]);
  };

  // ---------------------------------------------------- 视图：健康与指标

  routes.health = async () => {
    setCrumbs("健康与指标", "GET /healthz · /version · /metrics");
    const [health, version, metricsText] = await Promise.all([
      fetch("/healthz").then((r) => r.json()),
      fetch("/version").then((r) => r.json()),
      fetch("/metrics").then((r) => r.text()),
    ]);

    // 只挑出 Hub 自己的指标族，Prometheus 默认还会带一堆运行时噪音。
    const lines = metricsText.split("\n").filter((l) => l && !l.startsWith("#"));
    const hubLines = lines.filter((l) => l.startsWith("uenv_hub_"));
    const byRoute = new Map();
    for (const line of hubLines) {
      const m = line.match(/^uenv_hub_http_requests_total\{([^}]*)\}\s+(\d+(?:\.\d+)?)/);
      if (!m) continue;
      const labels = Object.fromEntries(
        m[1].split(",").map((kv2) => {
          const [k, v] = kv2.split("=");
          return [k, (v || "").replace(/"/g, "")];
        }),
      );
      const key = `${labels.method} ${labels.path}`;
      const entry = byRoute.get(key) || { total: 0, statuses: {} };
      entry.total += Number(m[2]);
      entry.statuses[labels.status] = (entry.statuses[labels.status] || 0) + Number(m[2]);
      byRoute.set(key, entry);
    }

    const routeRows = [...byRoute.entries()]
      .sort((a, b) => b[1].total - a[1].total)
      .map(([key, entry]) => ({
        cells: [
          el("code", { class: "mono", text: key }),
          fmtNum(entry.total),
          el(
            "span",
            {},
            Object.entries(entry.statuses)
              .sort()
              .map(([status, n]) =>
                badge(`${status}×${n}`, status.startsWith("2") ? "ok" : status.startsWith("4") ? "warn" : "bad"),
              ),
          ),
        ],
      }));

    return el("div", {}, [
      el("div", { class: "grid cols-3 section" }, [
        statTile("存活探针", health.status, `数据库 ${health.db}`),
        statTile("版本", version.version, version.git_sha || version.name),
        statTile("指标行数", fmtNum(hubLines.length), "uenv_hub_* 系列"),
      ]),
      card(
        "HTTP 请求分布",
        table([{ label: "方法与路径" }, { label: "累计请求", num: true }, "状态码"], routeRows, {
          empty: "还没有产生请求指标",
        }),
        { tight: true, hint: "来自 uenv_hub_http_requests_total" },
      ),
      el("div", { class: "section", style: "margin-top:18px" }, [
        card("原始指标输出", el("pre", { class: "json", text: metricsText })),
      ]),
    ]);
  };

  // ---------------------------------------------------- 视图：连接与凭据

  routes.settings = async () => {
    setCrumbs("连接与凭据");
    const input = el("input", {
      type: "password",
      placeholder: "uenvh_…",
      value: store.token,
      class: "grow",
      style: "font-family:var(--font-mono)",
    });
    const status = el("div", { style: "margin-top:12px" });

    const probe = async () => {
      status.replaceChildren(el("div", { class: "empty", text: "校验中…" }));
      try {
        const ov = await api("/api/v1/system/overview");
        state.overview = ov;
        applyOverviewChrome(ov);
        status.replaceChildren(
          el("div", {}, [
            el("p", { style: "margin:0 0 8px" }, [
              badge("凭据可用", "ok"),
              document.createTextNode(" 已成功读取总览接口。"),
            ]),
            kv([
              ["强制鉴权", ov.posture.require_token ? "是" : "否（开放模式，任何请求视为 admin）"],
              ["环境 / 包 / 栈", `${ov.registry.envs} / ${ov.registry.packages} / ${ov.registry.stacks}`],
            ]),
          ]),
        );
      } catch (error) {
        status.replaceChildren(renderError(error));
      }
    };

    const save = async () => {
      store.token = input.value.trim();
      $("#foot-token").textContent = store.token ? "已配置" : "未配置";
      toast(store.token ? "凭据已保存到本地浏览器" : "凭据已清除", "ok");
      await probe();
    };

    input.addEventListener("keydown", (e) => e.key === "Enter" && save());

    const endpointCard = card(
      "连接",
      kv([
        ["控制台来源", location.origin],
        ["API 基址", `${location.origin}/api/v1`],
        [
          "说明",
          "控制台由 Hub 自身提供，因此始终与所连 Hub 同源，不存在跨源与端点配置问题。",
        ],
      ]),
    );

    const tokenCard = card(
      "API 凭据",
      el("div", {}, [
        el("p", {
          style: "margin:0 0 10px;font-size:13px;color:var(--muted-foreground)",
          text:
            "凭据只保存在本浏览器的 localStorage，不会回传给 Hub 之外的任何地方。" +
            "只读浏览需要 reader，查看审计日志需要 admin。Hub 以 require_token = false 启动时可留空。",
        }),
        el("div", { class: "toolbar", style: "margin-bottom:0" }, [
          input,
          el("button", { class: "btn primary", text: "保存并校验", onclick: save }),
          el("button", {
            class: "btn",
            text: "清除",
            onclick: () => {
              input.value = "";
              save();
            },
          }),
        ]),
        status,
      ]),
    );

    const cliCard = card(
      "等效 CLI",
      el("pre", {
        class: "json",
        text: [
          `uenv hub login --endpoint ${location.origin} --token <TOKEN>`,
          "uenv hub status",
          "uenv env list",
          "uenv stack list",
          "uenv stack resolve <stack-id> --json",
          "uenv env sync <package-id> --version latest",
        ].join("\n"),
      }),
    );

    return el("div", { class: "grid cols-2" }, [tokenCard, el("div", {}, [endpointCard, el("div", { style: "height:14px" }), cliCard])]);
  };

  // ------------------------------------------------------------ 顶栏与轮询

  function applyOverviewChrome(ov) {
    $("#brand-version").textContent = `v${ov.service.version}${ov.service.git_sha ? ` · ${ov.service.git_sha.slice(0, 7)}` : ""}`;
    const byKind = ov.registry.packages_by_kind || {};
    // 侧栏计数要与各页实际列出的行数一致，否则数字本身就是误导：环境契约页
    // 只列在用契约，制品页统计的是产物而非包。
    $("#c-envs").textContent = ov.registry.active_envs ?? ov.registry.envs;
    $("#c-benchmarks").textContent = byKind.benchmark ?? 0;
    $("#c-scaffolds").textContent = byKind.agent_scaffold ?? ov.registry.agent_bridges;
    $("#c-artifacts").textContent = ov.registry.package_artifacts;
    $("#c-stacks").textContent = ov.registry.stacks;
    $("#c-templates").textContent = ov.registry.templates;
    const uptime = $("#uptime-pill");
    uptime.hidden = false;
    uptime.textContent = `uptime ${fmtDuration(ov.uptime_seconds)}`;
  }

  async function pollHealth() {
    const dot = $("#health-pill .dot");
    const text = $("#health-text");
    try {
      const resp = await fetch("/healthz");
      const body = await resp.json();
      const ok = resp.ok && body.status === "ok";
      dot.className = `dot ${ok ? "dot-ok" : "dot-bad"}`;
      text.textContent = ok ? `正常 · db ${body.db}` : `异常 · db ${body.db}`;
    } catch {
      dot.className = "dot dot-bad";
      text.textContent = "无法连接";
    }
  }

  function setAutoRefresh(on) {
    if (state.timer) {
      clearInterval(state.timer);
      state.timer = null;
    }
    localStorage.setItem("uenv-hub-console-autorefresh", on ? "1" : "0");
    if (on) state.timer = setInterval(() => router(), 10000);
  }

  // ------------------------------------------------------------------ 启动

  function boot() {
    $("#foot-endpoint").textContent = location.host;
    $("#foot-token").textContent = store.token ? "已配置" : "未配置";

    window.addEventListener("hashchange", router);
    $("#refresh-btn").addEventListener("click", () => router());

    const auto = $("#autorefresh");
    auto.checked = localStorage.getItem("uenv-hub-console-autorefresh") === "1";
    auto.addEventListener("change", () => setAutoRefresh(auto.checked));
    setAutoRefresh(auto.checked);

    pollHealth();
    setInterval(pollHealth, 15000);
    router();
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
})();
