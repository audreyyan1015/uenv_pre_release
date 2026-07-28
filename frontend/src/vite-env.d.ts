/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Server Obs REST/SSE 根地址，如 http://127.0.0.1:50053；留空则回落 fixture 演示模式。 */
  readonly VITE_AGGREGATION_BASE_URL?: string;
  /** Server Obs Bearer token；留空表示不鉴权（本地联调）。 */
  readonly VITE_AGGREGATION_TOKEN?: string;
  /** 未从 URL `?run=` 拿到 training_run_id 时的默认值。 */
  readonly VITE_DEFAULT_RUN_ID?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
