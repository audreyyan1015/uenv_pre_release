// @lovable.dev/vite-tanstack-config already includes the following — do NOT add them manually
// or the app will break with duplicate plugins:
//   - tanstackStart, viteReact, tailwindcss, tsConfigPaths, nitro (build-only using cloudflare as a default target),
//     componentTagger (dev-only), VITE_* env injection, @ path alias, React/TanStack dedupe,
//     error logger plugins, and sandbox detection (port/host/strictPort).
// You can pass additional config via defineConfig({ vite: { ... }, etc... }) if needed.
import { defineConfig } from "@lovable.dev/vite-tanstack-config";

// Keep the normal development proxy on the production read endpoint. Isolated
// pressure runs can override this process-local target without changing .env.
const obsProxyTarget = process.env.VITE_OBS_PROXY_TARGET ?? "http://127.0.0.1:50053";

export default defineConfig({
  vite: {
    server: {
      // 浏览器同源访问 /obs/*；压测可用 VITE_OBS_PROXY_TARGET 指向隔离 Obs。
      proxy: {
        "/obs": {
          target: obsProxyTarget,
          changeOrigin: true,
          rewrite: (path) => path.replace(/^\/obs/, ""),
        },
      },
    },
  },
  tanstackStart: {
    // Redirect TanStack Start's bundled server entry to src/server.ts (our SSR error wrapper).
    // nitro/vite builds from this
    server: { entry: "server" },
  },
});
