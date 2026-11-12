---
"tauri-bundler": "patch:enhance"
"tauri-cli": "patch:enhance"
"@tauri-apps/cli": "patch:enhance"
---

Verify the tools downloaded for AppImage bundling (AppRun, linuxdeploy and the type2 runtime) against pinned SHA-256 hashes, including copies already in the tools cache, and re-download cached tools that don't match. The type2 runtime now comes from a dated release and is passed to linuxdeploy via `LDAI_RUNTIME_FILE` instead of being fetched from the rolling `continuous` release, unless `LDAI_RUNTIME_FILE` is already set. The AppImage output plugin is no longer downloaded; linuxdeploy's built-in plugin is used, and a copy left in the tools cache by older versions is removed.
