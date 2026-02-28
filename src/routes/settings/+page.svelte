<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";

  interface AppConfig {
    config_version: number;
    hotkey: string;
    recording_mode: "toggle" | "push_to_talk";
    language: string;
    stt_model: string;
    enhance_model: string;
    enhance_enabled: boolean;
    vad_auto_stop: boolean;
    vad_silence_threshold_sec: number;
    vad_trim_silence: boolean;
    max_recording_duration_sec: number;
    min_recording_duration_ms: number;
    show_notifications: boolean;
    api_base_url: string;
    connect_timeout_sec: number;
    read_timeout_stt_sec: number;
    read_timeout_enhance_sec: number;
    retry_count: number;
    log_level: string;
    debug_save_audio: boolean;
  }

  type ApiKeyStatus = "idle" | "checking" | "valid" | "invalid";
  type SaveStatus = "idle" | "saving" | "saved" | "error";

  let config = $state<AppConfig | null>(null);
  let hasApiKey = $state(false);
  let apiKeyInput = $state("");
  let editingApiKey = $state(false);
  let apiKeyStatus = $state<ApiKeyStatus>("idle");
  let saveStatus = $state<SaveStatus>("idle");
  let statusMessage = $state("");
  let isOnboarding = $state(false);
  let loading = $state(true);

  onMount(async () => {
    try {
      config = await invoke<AppConfig>("get_config");
      hasApiKey = await invoke<boolean>("get_has_api_key");
      if (!hasApiKey) {
        editingApiKey = true;
      }
      isOnboarding =
        new URLSearchParams(window.location.search).has("onboarding") && !hasApiKey;
    } catch (e) {
      statusMessage = `Failed to load settings: ${e}`;
    } finally {
      loading = false;
    }
  });

  async function validateApiKey() {
    const key = apiKeyInput.trim();
    if (!key) return;
    apiKeyStatus = "checking";
    statusMessage = "";
    try {
      const valid = await invoke<boolean>("validate_api_key", { key });
      apiKeyStatus = valid ? "valid" : "invalid";
    } catch (e) {
      apiKeyStatus = "invalid";
      statusMessage = `Validation error: ${e}`;
    }
  }

  async function saveApiKey() {
    const key = apiKeyInput.trim();
    if (!key) return;
    try {
      await invoke("save_api_key", { key });
      hasApiKey = true;
      editingApiKey = false;
      apiKeyInput = "";
      apiKeyStatus = "idle";
      isOnboarding = false;
      showStatus("API key saved", "saved");
    } catch (e) {
      showStatus(`Failed to save API key: ${e}`, "error");
    }
  }

  async function handleSave() {
    if (!config) return;
    saveStatus = "saving";
    statusMessage = "";
    try {
      const current = await invoke<AppConfig>("get_config");
      const hotkeyChanged = current.hotkey !== config.hotkey;

      await invoke("save_config", { updatedConfig: config });

      if (hotkeyChanged) {
        try {
          await invoke("update_hotkey", { hotkeyStr: config.hotkey });
        } catch (e) {
          statusMessage = `Settings saved, but hotkey update failed: ${e}`;
          saveStatus = "error";
          return;
        }
      }

      showStatus("Settings saved", "saved");
    } catch (e) {
      saveStatus = "error";
      statusMessage = `Failed to save: ${e}`;
    }
  }

  async function handleReset() {
    try {
      const oldHotkey = config?.hotkey;
      config = await invoke<AppConfig>("reset_config");

      if (oldHotkey && config && oldHotkey !== config.hotkey) {
        try {
          await invoke("update_hotkey", { hotkeyStr: config.hotkey });
        } catch (e) {
          showStatus(`Reset done, but hotkey update failed: ${e}`, "error");
          return;
        }
      }

      showStatus("Reset to defaults", "saved");
    } catch (e) {
      showStatus(`Failed to reset: ${e}`, "error");
    }
  }

  function showStatus(message: string, status: SaveStatus) {
    saveStatus = status;
    statusMessage = message;
    setTimeout(() => {
      saveStatus = "idle";
      statusMessage = "";
    }, 3000);
  }
</script>

{#if loading}
  <div class="settings">
    <p class="loading">Loading settings...</p>
  </div>
{:else if config}
  <div class="settings">
    {#if isOnboarding}
      <div class="onboarding-banner">
        <h2>Welcome to VoiceDictator!</h2>
        <p>Enter your OpenAI API key to get started.</p>
      </div>
    {:else}
      <h1>Settings</h1>
    {/if}

    <!-- API Key -->
    <section class="section">
      <h3 class="section-title">API Key</h3>
      {#if hasApiKey && !editingApiKey}
        <div class="api-key-status">
          <span class="badge badge-success">API key is configured</span>
          <button
            class="btn btn-small"
            onclick={() => {
              editingApiKey = true;
            }}>Change</button
          >
        </div>
      {:else}
        <div class="field">
          <label for="api-key">OpenAI API Key</label>
          <input
            id="api-key"
            type="password"
            bind:value={apiKeyInput}
            placeholder="sk-..."
            autocomplete="off"
            autofocus={isOnboarding}
          />
        </div>
        <div class="api-key-actions">
          <button
            class="btn btn-secondary"
            onclick={validateApiKey}
            disabled={!apiKeyInput.trim() || apiKeyStatus === "checking"}
          >
            {apiKeyStatus === "checking" ? "Checking..." : "Validate"}
          </button>
          <button
            class="btn btn-primary"
            onclick={saveApiKey}
            disabled={!apiKeyInput.trim()}
          >
            Save Key
          </button>
          {#if hasApiKey}
            <button
              class="btn btn-small"
              onclick={() => {
                editingApiKey = false;
                apiKeyInput = "";
                apiKeyStatus = "idle";
              }}>Cancel</button
            >
          {/if}
        </div>
        {#if apiKeyStatus === "valid"}
          <p class="status-text status-success">API key is valid</p>
        {:else if apiKeyStatus === "invalid"}
          <p class="status-text status-error">API key is invalid</p>
        {/if}
      {/if}
    </section>

    <!-- Recording -->
    <section class="section">
      <h3 class="section-title">Recording</h3>
      <div class="field">
        <label for="hotkey">Hotkey</label>
        <input
          id="hotkey"
          type="text"
          bind:value={config.hotkey}
          placeholder="Ctrl+Shift+S"
        />
      </div>
      <div class="field">
        <span class="field-label">Recording Mode</span>
        <div class="radio-group">
          <label>
            <input
              type="radio"
              bind:group={config.recording_mode}
              value="toggle"
            />
            Toggle
          </label>
          <label>
            <input
              type="radio"
              bind:group={config.recording_mode}
              value="push_to_talk"
            />
            Push-to-talk
          </label>
        </div>
      </div>
      <div class="field">
        <label for="max-duration">Max Recording Duration (sec)</label>
        <input
          id="max-duration"
          type="number"
          bind:value={config.max_recording_duration_sec}
          min="10"
          max="120"
        />
      </div>
    </section>

    <!-- Speech Recognition -->
    <section class="section">
      <h3 class="section-title">Speech Recognition</h3>
      <div class="field">
        <label for="language">Language</label>
        <select id="language" bind:value={config.language}>
          <option value="auto">Auto</option>
          <option value="ru">Russian</option>
          <option value="en">English</option>
        </select>
      </div>
      <div class="field">
        <label for="stt-model">STT Model</label>
        <input
          id="stt-model"
          type="text"
          bind:value={config.stt_model}
        />
      </div>
    </section>

    <!-- Text Enhancement -->
    <section class="section">
      <h3 class="section-title">Text Enhancement</h3>
      <div class="field checkbox">
        <label>
          <input type="checkbox" bind:checked={config.enhance_enabled} />
          Enable text enhancement
        </label>
      </div>
      {#if config.enhance_enabled}
        <div class="field">
          <label for="enhance-model">Enhance Model</label>
          <input
            id="enhance-model"
            type="text"
            bind:value={config.enhance_model}
          />
        </div>
      {/if}
    </section>

    <!-- VAD -->
    <section class="section">
      <h3 class="section-title">Voice Activity Detection</h3>
      <div class="field checkbox">
        <label>
          <input type="checkbox" bind:checked={config.vad_auto_stop} />
          Auto-stop on silence
        </label>
      </div>
      {#if config.vad_auto_stop}
        <div class="field">
          <label for="vad-threshold">Silence Threshold (sec)</label>
          <input
            id="vad-threshold"
            type="number"
            bind:value={config.vad_silence_threshold_sec}
            min="1"
            max="30"
            step="0.5"
          />
        </div>
      {/if}
      <div class="field checkbox">
        <label>
          <input type="checkbox" bind:checked={config.vad_trim_silence} />
          Trim silence from audio
        </label>
      </div>
    </section>

    <!-- Notifications -->
    <section class="section">
      <h3 class="section-title">Notifications</h3>
      <div class="field checkbox">
        <label>
          <input type="checkbox" bind:checked={config.show_notifications} />
          Show notifications
        </label>
      </div>
    </section>

    <!-- Status message -->
    {#if statusMessage}
      <p
        class="status-text {saveStatus === 'error'
          ? 'status-error'
          : 'status-success'}"
      >
        {statusMessage}
      </p>
    {/if}

    <!-- Actions -->
    <div class="actions">
      <button
        class="btn btn-primary"
        onclick={handleSave}
        disabled={saveStatus === "saving"}
      >
        {saveStatus === "saving" ? "Saving..." : "Save"}
      </button>
      <button class="btn btn-secondary" onclick={handleReset}>
        Reset to Defaults
      </button>
    </div>
  </div>
{:else}
  <div class="settings">
    <p class="status-text status-error">
      Failed to load settings. {statusMessage}
    </p>
  </div>
{/if}

<style>
  .settings {
    max-width: 480px;
    margin: 0 auto;
    padding: 20px 24px;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Oxygen,
      sans-serif;
    font-size: 14px;
    color: #1a1a1a;
  }

  h1 {
    font-size: 20px;
    font-weight: 600;
    margin: 0 0 20px;
  }

  .onboarding-banner {
    background: #e8f4fd;
    border: 1px solid #b3d9f2;
    border-radius: 6px;
    padding: 16px;
    margin-bottom: 20px;
  }

  .onboarding-banner h2 {
    font-size: 18px;
    margin: 0 0 4px;
  }

  .onboarding-banner p {
    margin: 0;
    color: #555;
  }

  .section {
    margin-bottom: 20px;
    padding-bottom: 16px;
    border-bottom: 1px solid #e5e5e5;
  }

  .section-title {
    font-size: 12px;
    font-weight: 600;
    color: #888;
    text-transform: uppercase;
    letter-spacing: 0.8px;
    margin: 0 0 10px;
  }

  .field {
    margin-bottom: 10px;
  }

  .field > label,
  .field-label {
    display: block;
    font-size: 13px;
    color: #444;
    margin-bottom: 4px;
    font-weight: 500;
  }

  input[type="text"],
  input[type="password"],
  input[type="number"],
  select {
    width: 100%;
    padding: 7px 10px;
    border: 1px solid #ccc;
    border-radius: 4px;
    font-size: 13px;
    background: #fff;
    box-sizing: border-box;
  }

  input[type="text"]:focus,
  input[type="password"]:focus,
  input[type="number"]:focus,
  select:focus {
    border-color: #4a9eff;
    outline: none;
    box-shadow: 0 0 0 2px rgba(74, 158, 255, 0.2);
  }

  .checkbox label {
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
    font-size: 13px;
    color: #444;
  }

  .radio-group {
    display: flex;
    gap: 20px;
  }

  .radio-group label {
    display: flex;
    align-items: center;
    gap: 4px;
    cursor: pointer;
    font-size: 13px;
    color: #444;
  }

  .api-key-status {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .api-key-actions {
    display: flex;
    gap: 8px;
    margin-top: 8px;
    align-items: center;
  }

  .badge {
    display: inline-block;
    padding: 4px 10px;
    border-radius: 12px;
    font-size: 12px;
    font-weight: 500;
  }

  .badge-success {
    background: #e6f4ea;
    color: #1e7e34;
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

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-primary {
    background: #4a9eff;
    color: white;
  }

  .btn-primary:hover:not(:disabled) {
    background: #3a8eef;
  }

  .btn-secondary {
    background: #f0f0f0;
    color: #333;
    border-color: #ccc;
  }

  .btn-secondary:hover:not(:disabled) {
    background: #e4e4e4;
  }

  .btn-small {
    padding: 4px 10px;
    font-size: 12px;
    background: #f0f0f0;
    color: #333;
    border-color: #ccc;
  }

  .btn-small:hover {
    background: #e4e4e4;
  }

  .status-text {
    font-size: 13px;
    margin: 8px 0;
  }

  .status-success {
    color: #1e7e34;
  }

  .status-error {
    color: #dc3545;
  }

  .actions {
    display: flex;
    gap: 10px;
    margin-top: 16px;
    padding-top: 16px;
    border-top: 1px solid #e5e5e5;
  }

  .loading {
    color: #888;
    text-align: center;
    padding: 40px 0;
  }
</style>
