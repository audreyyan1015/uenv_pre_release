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
  /** Hub 控制台外链；不配置时使用同源 /hub/console 代理。 */
  readonly VITE_HUB_CONSOLE_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
