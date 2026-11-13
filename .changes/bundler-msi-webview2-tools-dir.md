---
"tauri-bundler": "patch:enhance"
"tauri-cli": "patch:enhance"
"@tauri-apps/cli": "patch:enhance"
---

The MSI bundler now keeps the downloaded WebView2 offline installer and bootstrapper in the tools directory, like the NSIS bundler, instead of re-downloading them on every build. WebView2 installers are written atomically, so an interrupted download is no longer reused, and outdated offline installers are removed when a new one is downloaded.
