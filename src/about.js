const { invoke } = window.__TAURI__.core;

function openUrl(url) {
  if (window.__TAURI__?.shell?.open) {
    window.__TAURI__.shell.open(url);
  } else {
    window.open(url, "_blank");
  }
}

window.addEventListener("DOMContentLoaded", async () => {
  try {
    const version = await invoke("get_app_version");
    document.getElementById("version").textContent = "v" + version;
  } catch (_) {}

  document.getElementById("link-github").addEventListener("click", (e) => {
    e.preventDefault();
    openUrl("https://github.com/shoootyou/non-sleep-please");
  });
  document.getElementById("link-license").addEventListener("click", (e) => {
    e.preventDefault();
    openUrl("https://github.com/shoootyou/non-sleep-please/blob/main/LICENSE");
  });
});
