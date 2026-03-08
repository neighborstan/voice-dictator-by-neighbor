<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";

  let text = $state("");
  let autoClose = $state(true);
  let countdown = $state(10);
  let copied = $state(false);
  let timer: ReturnType<typeof setInterval> | null = null;
  let unlisten: (() => void) | null = null;

  async function loadText() {
    try {
      const result = await invoke<string | null>("get_result_text");
      text = result ?? "";
    } catch (e) {
      text = `Failed to load text: ${e}`;
    }
    resetCountdown();
  }

  function resetCountdown() {
    countdown = 10;
    copied = false;
    stopTimer();
    if (autoClose) startTimer();
  }

  function startTimer() {
    stopTimer();
    timer = setInterval(() => {
      countdown -= 1;
      if (countdown <= 0) {
        closeWindow();
      }
    }, 1000);
  }

  function stopTimer() {
    if (timer !== null) {
      clearInterval(timer);
      timer = null;
    }
  }

  async function copyText() {
    try {
      await navigator.clipboard.writeText(text);
      copied = true;
      setTimeout(() => {
        copied = false;
      }, 2000);
    } catch {
      // Browser clipboard API unavailable in this context
    }
  }

  async function closeWindow() {
    stopTimer();
    try {
      await getCurrentWindow().close();
    } catch {
      // Window might already be closing
    }
  }

  function handleAutoCloseChange(e: Event) {
    const target = e.target as HTMLInputElement;
    autoClose = target.checked;
    if (autoClose) {
      resetCountdown();
    } else {
      stopTimer();
    }
  }

  onMount(async () => {
    await loadText();
    unlisten = await listen("result-text-updated", () => {
      loadText();
    });
  });

  onDestroy(() => {
    stopTimer();
    if (unlisten) unlisten();
  });
</script>

<div class="result">
  <h2>Dictation Result</h2>
  <textarea readonly class="text-area">{text}</textarea>
  <div class="controls">
    <button class="btn btn-primary" onclick={copyText}>
      {copied ? "Copied!" : "Copy"}
    </button>
    <button class="btn btn-secondary" onclick={closeWindow}>Close</button>
  </div>
  <div class="auto-close">
    <label>
      <input
        type="checkbox"
        checked={autoClose}
        onchange={handleAutoCloseChange}
      />
      Auto-close after {countdown}s
    </label>
  </div>
</div>

<style>
  .result {
    font-family: -apple-system, system-ui, "Segoe UI", sans-serif;
    padding: 16px;
    display: flex;
    flex-direction: column;
    height: 100vh;
    box-sizing: border-box;
  }

  h2 {
    font-size: 16px;
    font-weight: 600;
    margin: 0 0 12px;
    color: #333;
  }

  .text-area {
    flex: 1;
    width: 100%;
    resize: none;
    border: 1px solid #ccc;
    border-radius: 4px;
    padding: 10px;
    font-size: 14px;
    line-height: 1.5;
    font-family: inherit;
    background: #fafafa;
    box-sizing: border-box;
    color: #333;
  }

  .text-area:focus {
    outline: none;
    border-color: #4a9eff;
    box-shadow: 0 0 0 2px rgba(74, 158, 255, 0.2);
  }

  .controls {
    display: flex;
    gap: 8px;
    margin-top: 12px;
  }

  .auto-close {
    margin-top: 8px;
    font-size: 12px;
    color: #888;
  }

  .auto-close label {
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
  }

  .btn {
    padding: 8px 16px;
    border: 1px solid transparent;
    border-radius: 4px;
    cursor: pointer;
    font-size: 13px;
    font-weight: 500;
    transition: background 0.15s;
  }

  .btn-primary {
    background: #4a9eff;
    color: white;
  }

  .btn-primary:hover {
    background: #3a8eef;
  }

  .btn-secondary {
    background: #f0f0f0;
    color: #333;
    border-color: #ccc;
  }

  .btn-secondary:hover {
    background: #e4e4e4;
  }
</style>
