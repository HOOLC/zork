// Captured host functions are not exposed as a second, unvalidated API.
((native, log) => {
  delete globalThis.__androidCall;
  delete globalThis.__localLog;
  const call = async (method, args = {}) => {
    const reply = JSON.parse(native(JSON.stringify({ ...args, method })));
    if (reply.error) throw new Error(reply.error);
    return reply.value;
  };
  const output = (...values) => {
    const error = log(values.map((v) => (typeof v === "string" ? v : JSON.stringify(v))).join(" "));
    if (error) throw new Error(error);
  };
  globalThis.console = Object.freeze({ log: output, warn: output, error: output });
  globalThis.android = Object.freeze({
    info: () => call("info"),
    startActivity: (intent) => call("start_activity", { intent, result: false }),
    startActivityForResult: (intent) => call("start_activity", { intent, result: true }),
    requestPermissions: (permissions) => call("request_permissions", { permissions }),
    hasPermission: (permission) => call("has_permission", { permission }),
    clipboard: Object.freeze({
      write: (text) => call("clipboard_write", { text }),
      read: () => call("clipboard_read"),
      clear: () => call("clipboard_clear"),
    }),
    torch: (enabled, cameraId = null) => call("torch", { enabled, camera_id: cameraId }),
    volume: Object.freeze({
      get: (stream = "music") => call("volume_get", { stream }),
      set: (index, stream = "music", showUi = true) =>
        call("volume_set", { index, stream, show_ui: showUi }),
    }),
    vibrate: (milliseconds) => call("vibrate", { milliseconds }),
    settings: Object.freeze({
      get: (namespace, key) => call("settings_get", { namespace, key }),
      canWrite: () => call("settings_can_write"),
      put: (key, value) => call("settings_put", { key, value }),
    }),
    content: Object.freeze({
      readText: (uri) => call("content_read_text", { uri }),
      writeText: (uri, text) => call("content_write_text", { uri, text }),
    }),
    location: () => call("location"),
    sleep: (milliseconds) => call("sleep", { milliseconds }),
  });
})(globalThis.__androidCall, globalThis.__localLog);
