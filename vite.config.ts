import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const port = Number(process.env.PORT ?? 1420);

export default defineConfig({
  root: "app",
  plugins: [react()],
  clearScreen: false,
  server: { port, strictPort: true, host: "localhost" },
  build: { outDir: "../dist", emptyOutDir: true, target: "safari15" },
});
