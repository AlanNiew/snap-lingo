import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath, URL } from "node:url";

const page = (p: string) => fileURLToPath(new URL(p, import.meta.url));

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],

  // Surfaced in 设置 → 关于 so a screenshot of the About tab identifies a build.
  define: {
    __APP_VERSION__: JSON.stringify(process.env.npm_package_version ?? "1.0.0"),
  },

  // Tauri expects a fixed port and fails if it is not available.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    watch: {
      // Rust sources are watched by cargo, not vite.
      ignored: ["**/src-tauri/**"],
    },
  },

  build: {
    // Tauri uses a modern webview (WebView2 on Windows).
    target: "chrome110",
    minify: "esbuild",
    sourcemap: false,
    rollupOptions: {
      input: {
        main: page("index.html"),
        overlay: page("overlay.html"),
        result: page("result.html"),
      },
    },
  },
});
