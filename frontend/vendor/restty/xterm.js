import {
  createRestty
} from "./chunk-meqn8xtd.js";

// src/xterm/app-options.ts
function createCompatAppOptions(userAppOptions, emitData) {
  return (context) => {
    const resolved = typeof userAppOptions === "function" ? userAppOptions(context) : userAppOptions ?? {};
    const userBeforeInput = resolved.beforeInput;
    return {
      ...resolved,
      beforeInput: ({ text, source }) => {
        const maybeNext = userBeforeInput?.({ text, source });
        if (maybeNext === null)
          return null;
        const nextText = maybeNext === undefined ? text : maybeNext;
        if (source !== "pty" && nextText) {
          emitData(nextText);
        }
        return nextText;
      }
    };
  };
}

// src/xterm/dimensions.ts
function normalizeDimension(value, fallback) {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0)
    return fallback;
  return Math.max(1, Math.trunc(value));
}

// src/xterm/listeners.ts
function addListener(bucket, listener) {
  bucket.add(listener);
  return {
    dispose: () => {
      bucket.delete(listener);
    }
  };
}
function emitWithGuard(bucket, payload, label) {
  const listeners = Array.from(bucket);
  for (let i = 0;i < listeners.length; i += 1) {
    try {
      listeners[i](payload);
    } catch (error) {
      console.error(`[restty/xterm] ${label} listener error:`, error);
    }
  }
}

// src/xterm.ts
class Terminal {
  resttyOptionsBase;
  userAppOptions;
  addons = new Set;
  pendingOutput = [];
  dataListeners = new Set;
  resizeListeners = new Set;
  optionValues;
  resttyInstance = null;
  elementRef = null;
  disposed = false;
  opened = false;
  pendingSize = null;
  cols;
  rows;
  constructor(options = {}) {
    const { cols, rows, appOptions, ...resttyOptionsBase } = options;
    this.resttyOptionsBase = resttyOptionsBase;
    this.userAppOptions = appOptions;
    this.optionValues = { ...options };
    delete this.optionValues.cols;
    delete this.optionValues.rows;
    this.cols = normalizeDimension(cols, 80);
    this.rows = normalizeDimension(rows, 24);
    if (Number.isFinite(cols) && Number.isFinite(rows)) {
      this.pendingSize = { cols: this.cols, rows: this.rows };
    }
  }
  get element() {
    return this.elementRef;
  }
  get restty() {
    return this.resttyInstance;
  }
  get options() {
    return {
      ...this.optionValues,
      cols: this.cols,
      rows: this.rows
    };
  }
  set options(next) {
    this.ensureUsable();
    this.applyOptions(next);
  }
  open(parent) {
    this.ensureUsable();
    if (this.opened) {
      throw new Error("xterm compatibility Terminal is already opened");
    }
    this.opened = true;
    this.elementRef = parent;
    this.resttyInstance = createRestty({
      ...this.resttyOptionsBase,
      appOptions: createCompatAppOptions(this.userAppOptions, (data) => {
        emitWithGuard(this.dataListeners, data, "onData");
      }),
      root: parent
    });
    if (this.pendingSize) {
      this.resttyInstance.resize(this.pendingSize.cols, this.pendingSize.rows);
    }
    if (this.pendingOutput.length > 0) {
      for (let i = 0;i < this.pendingOutput.length; i += 1) {
        this.resttyInstance.sendInput(this.pendingOutput[i], "pty");
      }
      this.pendingOutput.length = 0;
    }
  }
  write(data, callback) {
    this.ensureUsable();
    if (!data) {
      callback?.();
      return;
    }
    if (this.resttyInstance) {
      this.resttyInstance.sendInput(data, "pty");
    } else {
      this.pendingOutput.push(data);
    }
    callback?.();
  }
  writeln(data = "", callback) {
    this.write(`${data}\r
`, callback);
  }
  resize(cols, rows) {
    this.ensureUsable();
    const next = {
      cols: normalizeDimension(cols, this.cols),
      rows: normalizeDimension(rows, this.rows)
    };
    this.cols = next.cols;
    this.rows = next.rows;
    this.pendingSize = next;
    this.resttyInstance?.resize(next.cols, next.rows);
    emitWithGuard(this.resizeListeners, next, "onResize");
  }
  focus() {
    if (this.disposed)
      return;
    this.resttyInstance?.focus();
  }
  blur() {
    if (this.disposed)
      return;
    this.resttyInstance?.blur();
  }
  clear() {
    this.ensureUsable();
    if (this.resttyInstance) {
      this.resttyInstance.clearScreen();
      return;
    }
    this.pendingOutput.length = 0;
  }
  reset() {
    this.ensureUsable();
    this.clear();
    if (this.resttyInstance) {
      this.resttyInstance.sendInput("\x1Bc", "pty");
    }
  }
  onData(listener) {
    this.ensureUsable();
    return addListener(this.dataListeners, listener);
  }
  onResize(listener) {
    this.ensureUsable();
    return addListener(this.resizeListeners, listener);
  }
  setOption(key, value) {
    this.ensureUsable();
    this.applyOptions({ [key]: value });
  }
  getOption(key) {
    if (key === "cols")
      return this.cols;
    if (key === "rows")
      return this.rows;
    return this.optionValues[key];
  }
  loadAddon(addon) {
    this.ensureUsable();
    if (!addon || typeof addon.activate !== "function" || typeof addon.dispose !== "function") {
      throw new Error("xterm compatibility addon must define activate() and dispose()");
    }
    if (this.addons.has(addon))
      return;
    addon.activate(this);
    this.addons.add(addon);
  }
  dispose() {
    if (this.disposed)
      return;
    this.disposed = true;
    const addons = Array.from(this.addons);
    this.addons.clear();
    for (let i = 0;i < addons.length; i += 1) {
      try {
        addons[i].dispose();
      } catch {}
    }
    this.pendingOutput.length = 0;
    this.pendingSize = null;
    this.opened = false;
    this.elementRef = null;
    this.dataListeners.clear();
    this.resizeListeners.clear();
    if (this.resttyInstance) {
      this.resttyInstance.destroy();
      this.resttyInstance = null;
    }
  }
  ensureUsable() {
    if (this.disposed) {
      throw new Error("xterm compatibility Terminal is disposed");
    }
  }
  applyOptions(next) {
    const hasCols = Object.prototype.hasOwnProperty.call(next, "cols");
    const hasRows = Object.prototype.hasOwnProperty.call(next, "rows");
    if (hasCols || hasRows) {
      const cols = hasCols ? normalizeDimension(next.cols, this.cols) : this.cols;
      const rows = hasRows ? normalizeDimension(next.rows, this.rows) : this.rows;
      this.resize(cols, rows);
    }
    const keys = Object.keys(next);
    for (let i = 0;i < keys.length; i += 1) {
      const key = keys[i];
      if (key === "cols" || key === "rows")
        continue;
      this.optionValues[key] = next[key];
    }
  }
}
export {
  Terminal
};
