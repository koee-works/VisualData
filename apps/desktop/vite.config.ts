import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";

// Tauri expects a fixed dev port and serves the built files from dist/.
export default defineConfig({
  plugins: [tailwindcss(), svelte()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: { target: "safari16", outDir: "dist", chunkSizeWarningLimit: 2000 },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
  },
  resolve: process.env.VITEST ? { conditions: ["browser"] } : undefined,
});
