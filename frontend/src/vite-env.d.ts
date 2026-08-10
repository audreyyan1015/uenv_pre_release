/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Server Obs REST/SSE 根地址，如 http://127.0.0.1:50053；留空则回落 fixture 演示模式。 */
  readonly VITE_AGGREGATION_BASE_URL?: string;
  /** Server Obs Bearer token；留空表示不鉴权（本地联调）。 */
  readonly VITE_AGGREGATION_TOKEN?: string;
  /** 未从 URL `?run=` 拿到 training_run_id 时的默认值。 */
  readonly VITE_DEFAULT_RUN_ID?: string;
  /** Hub 只读 overview token。仅用于本地联调；生产环境应由服务端代理注入。 */
  readonly VITE_HUB_TOKEN?: string;
  /**
   * Hub 控制台外链（须为 Hub 源站，如 http://127.0.0.1:8088/ 或
   * http://8.130.95.176:8088/console）。勿使用 /hub/console 代理路径：
   * console 的 CSS/JS/API 为绝对路径，经代理打开会丢样式。
   */
  readonly VITE_HUB_CONSOLE_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
