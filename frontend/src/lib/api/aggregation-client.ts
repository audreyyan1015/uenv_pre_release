import type { ChainState, RunStatusPayload, StateDelta } from "@/lib/types/chain-state";

export interface AggregationConfig {
  /** 已去掉末尾斜杠的 Obs 根地址；为 null 表示未配置，前端应回落 fixture 演示模式。 */
  baseUrl: string | null;
  token?: string;
  /** 无 baseUrl 时为 true：调用方应使用 `src/lib/api/fixture.ts` 而不是真的建 client。 */
  useFixture: boolean;
}

/**
 * 读取 `VITE_AGGREGATION_BASE_URL` / `VITE_AGGREGATION_TOKEN`。
 * 未配置根地址时视为 FE-0 离线演示模式（fixture），不尝试联网。
 */
export function getAggregationConfig(): AggregationConfig {
  const rawBaseUrl = import.meta.env.VITE_AGGREGATION_BASE_URL?.trim();
  const token = import.meta.env.VITE_AGGREGATION_TOKEN?.trim() || undefined;
  if (!rawBaseUrl) {
    return { baseUrl: null, token, useFixture: true };
  }
  return { baseUrl: rawBaseUrl.replace(/\/+$/, ""), token, useFixture: false };
}

export interface StreamHandlers {
  onOpen?: () => void;
  onFullState?: (state: ChainState) => void;
  onStateDelta?: (delta: StateDelta) => void;
  onRunStatus?: (payload: RunStatusPayload) => void;
  onPing?: () => void;
  /** SSE 层错误（连接失败/被服务端关闭）；不代表致命错误，调用方通常应重连。 */
  onError?: (error: unknown) => void;
}

export interface SubscribeStreamOptions {
  /** 断线重连时用于续传；浏览器原生重连会自动带 Last-Event-ID 请求头，
   * 这里额外通过查询参数带一份，方便我们手动重建 EventSource 时也能续传。 */
  lastEventId?: string;
  /** 传入后 abort 即关闭当前 SSE 连接，是本客户端唯一的“取消订阅”方式。 */
  signal?: AbortSignal;
}

const SSE_EVENT_TYPES = ["full_state", "state_delta", "run_status", "ping"] as const;

/**
 * Server Obs REST + SSE 客户端。
 *
 * 鉴权说明：原生 `EventSource` 无法自定义请求头（不能设 `Authorization`），
 * 因此当配置了 token 时，通过查询参数 `?token=` 传递；未配置 token（本地联调 /
 * 无鉴权部署）时不带该参数。REST 请求（`getState`）走 `fetch`，可以正常使用
 * `Authorization: Bearer <token>` 头。
 */
export class AggregationClient {
  private readonly baseUrl: string;
  private readonly token?: string;

  constructor(baseUrl: string, token?: string) {
    this.baseUrl = baseUrl.replace(/\/+$/, "");
    this.token = token;
  }

  private authHeaders(): Record<string, string> {
    return this.token ? { Authorization: `Bearer ${this.token}` } : {};
  }

  async getState(runId: string): Promise<ChainState> {
    const res = await fetch(`${this.baseUrl}/api/v1/runs/${encodeURIComponent(runId)}/state`, {
      headers: { ...this.authHeaders() },
    });
    if (!res.ok) {
      throw new Error(`获取 ChainState 失败：HTTP ${res.status}`);
    }
    return (await res.json()) as ChainState;
  }

  /**
   * 订阅 `GET .../stream`。没有返回值，取消订阅请通过 `opts.signal` abort。
   * 处理 SSE `event:` 字段区分的四类载荷；若服务端未设置 `event:`（走默认
   * `message` 事件），会尝试从 JSON body 里的 `type` 字段兜底分派。
   */
  subscribeStream(
    runId: string,
    handlers: StreamHandlers,
    opts: SubscribeStreamOptions = {},
  ): void {
    const streamUrl = `${this.baseUrl}/api/v1/runs/${encodeURIComponent(runId)}/stream`;
    // The local frontend uses the same-origin `/obs` proxy. `new URL("/obs/...")`
    // without a base throws in browsers, while an absolute configured base URL is
    // already valid. Supplying the current origin supports both forms.
    const url = new URL(streamUrl, window.location.origin);
    if (this.token) url.searchParams.set("token", this.token);
    if (opts.lastEventId) url.searchParams.set("last_event_id", opts.lastEventId);

    const source = new EventSource(url.toString());

    const close = () => {
      source.close();
    };

    if (opts.signal) {
      if (opts.signal.aborted) {
        close();
        return;
      }
      opts.signal.addEventListener("abort", close, { once: true });
    }

    source.addEventListener("open", () => handlers.onOpen?.());
    source.addEventListener("error", (event) => handlers.onError?.(event));

    const parse = <T>(raw: string): T | null => {
      try {
        return JSON.parse(raw) as T;
      } catch {
        return null;
      }
    };

    source.addEventListener("full_state", (event) => {
      const data = parse<ChainState>((event as MessageEvent).data);
      if (data) handlers.onFullState?.(data);
    });
    source.addEventListener("state_delta", (event) => {
      const data = parse<StateDelta>((event as MessageEvent).data);
      if (data) handlers.onStateDelta?.(data);
    });
    source.addEventListener("run_status", (event) => {
      const data = parse<RunStatusPayload>((event as MessageEvent).data);
      if (data) handlers.onRunStatus?.(data);
    });
    source.addEventListener("ping", () => handlers.onPing?.());

    // 兜底：未显式设置 `event:` 字段时走默认 message，按 body.type 分派。
    source.addEventListener("message", (event) => {
      const data = parse<{ type?: (typeof SSE_EVENT_TYPES)[number] } & Record<string, unknown>>(
        (event as MessageEvent).data,
      );
      if (!data || !data.type) return;
      switch (data.type) {
        case "full_state":
          handlers.onFullState?.(data as unknown as ChainState);
          break;
        case "state_delta":
          handlers.onStateDelta?.(data as unknown as StateDelta);
          break;
        case "run_status":
          handlers.onRunStatus?.(data as unknown as RunStatusPayload);
          break;
        case "ping":
          handlers.onPing?.();
          break;
        default:
          break;
      }
    });
  }
}
